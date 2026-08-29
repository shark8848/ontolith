//! L6 reasoning integration for the L5 gateway (P6-03): configurable
//! inference mode + materialization guards, wired into the shared SPARQL
//! execution path through a read-service overlay.

use ontolith_core::domain::{GraphId, Iri, NodeId, ObjectId, OntologyObject, TimestampMs};
use ontolith_core::error::OntolithError;
use ontolith_query::application::QueryReadService;
use ontolith_query::domain::{QueryPlanId, TenantScope};
use ontolith_rdf::domain::{Term, Triple};
use ontolith_reasoner::domain::{
    InferenceMode, OntologyGraphReader, ReasoningTask, load_ontology_payload,
};
use ontolith_storage::application::{
    DictionaryCodec, QuadRepository, StorageEngine, TripleRepository,
};
use ontolith_storage::infrastructure::EngineQuadRepository;
use ontolith_transaction::domain::TxnId;
use std::env;
use std::sync::Arc;

const INFERENCE_MODE_ENV: &str = "ONTOLITH_INFERENCE_MODE";
const INFERENCE_MAX_ITERATIONS_ENV: &str = "ONTOLITH_INFERENCE_MAX_ITERATIONS";
const INFERENCE_MAX_ELAPSED_MS_ENV: &str = "ONTOLITH_INFERENCE_MAX_ELAPSED_MS";

const ONTOLOGY_TBOX_ENV: &str = "ONTOLITH_ONTOLOGY_TBOX";
const ONTOLOGY_ABOX_ENV: &str = "ONTOLITH_ONTOLOGY_ABOX";
const ONTOLOGY_ANNOTATION_ENV: &str = "ONTOLITH_ONTOLOGY_ANNOTATION";
const ONTOLOGY_RULE_ENV: &str = "ONTOLITH_ONTOLOGY_RULE";
const ONTOLOGY_PROVENANCE_ENV: &str = "ONTOLITH_ONTOLOGY_PROVENANCE";

const DEFAULT_MAX_ITERATIONS: u32 = 64;

/// Server-level reasoning posture (P6-03): the selected inference mode plus
/// the materialization guards (iteration cap and wall-clock budget) that
/// protect the query hot path from unbounded deep reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InferenceConfig {
    pub mode: InferenceMode,
    pub max_iterations: u32,
    pub max_elapsed_ms: Option<u64>,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            mode: InferenceMode::Off,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            max_elapsed_ms: None,
        }
    }
}

impl InferenceConfig {
    pub fn new(mode: InferenceMode, max_iterations: u32, max_elapsed_ms: Option<u64>) -> Self {
        Self {
            mode,
            max_iterations,
            max_elapsed_ms,
        }
    }

    pub const fn is_enabled(&self) -> bool {
        self.mode.is_enabled()
    }

    /// Load the inference posture from the environment contract:
    /// `ONTOLITH_INFERENCE_MODE` (off|forward|hybrid, default `off`),
    /// `ONTOLITH_INFERENCE_MAX_ITERATIONS` (default 64),
    /// `ONTOLITH_INFERENCE_MAX_ELAPSED_MS` (default unlimited).
    pub fn from_env() -> Self {
        let mode = env::var(INFERENCE_MODE_ENV)
            .ok()
            .as_deref()
            .and_then(InferenceMode::parse)
            .unwrap_or(InferenceMode::Off);
        let max_iterations = env::var(INFERENCE_MAX_ITERATIONS_ENV)
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_MAX_ITERATIONS);
        let max_elapsed_ms = env::var(INFERENCE_MAX_ELAPSED_MS_ENV)
            .ok()
            .and_then(|v| v.parse().ok());
        Self {
            mode,
            max_iterations,
            max_elapsed_ms,
        }
    }

    /// Apply a per-request mode override (`?inference=off|forward|hybrid`);
    /// the guard budgets remain the server-configured values.
    pub fn with_override(&self, raw: &str) -> Result<Self, OntolithError> {
        let mode = InferenceMode::parse(raw).ok_or(OntolithError::InvalidArgument(
            "invalid inference mode (expected off|forward|hybrid)",
        ))?;
        Ok(Self { mode, ..*self })
    }

    pub fn reasoning_task(&self, plan_id: Option<QueryPlanId>) -> ReasoningTask {
        ReasoningTask {
            plan_id,
            mode: self.mode,
            max_iterations: self.max_iterations,
            max_elapsed_ms: self.max_elapsed_ms,
        }
    }
}

/// The storage read service the query pipeline would otherwise build.
pub fn base_read_service(
    triples: Arc<dyn TripleRepository>,
    dictionary: Arc<dyn DictionaryCodec>,
    storage: Arc<dyn StorageEngine>,
) -> Arc<dyn QueryReadService> {
    let quads: Arc<dyn QuadRepository> = Arc::new(EngineQuadRepository::new(storage));
    Arc::new(
        ontolith_query::infrastructure::InMemoryQueryReadService::with_quads(
            triples,
            Some(dictionary),
            quads,
        ),
    )
}

/// Materialization input: all triples, or (enforced tenant mode) the union of
/// triples in the caller's owned named graphs so inference never observes
/// cross-tenant data.
pub fn reasoning_input(
    base: &dyn QueryReadService,
    scope: Option<&TenantScope>,
) -> Result<Vec<Triple>, OntolithError> {
    reasoning_input_with_ontology(base, scope, None)
}

/// Materialization input with the configured ontology payload (P1-01): the
/// tenant-scoped triples plus every role graph referenced by `ontology`.
/// Shared ontology graphs are included regardless of tenant ownership so a
/// single ontology can be reasoned over by multiple tenants.
pub fn reasoning_input_with_ontology(
    base: &dyn QueryReadService,
    scope: Option<&TenantScope>,
    ontology: Option<&OntologyObject>,
) -> Result<Vec<Triple>, OntolithError> {
    let mut out = tenant_scoped_input(base, scope)?;
    if let Some(ontology) = ontology {
        let reader = QueryReadOntologyReader { base };
        out.extend(load_ontology_payload(&reader, ontology)?.all());
    }
    Ok(out)
}

fn tenant_scoped_input(
    base: &dyn QueryReadService,
    scope: Option<&TenantScope>,
) -> Result<Vec<Triple>, OntolithError> {
    let Some(scope) = scope else {
        return base.all_triples(None);
    };
    let mut out = Vec::new();
    for graph in base.named_graph_names(None) {
        if scope.is_owned(graph.as_str()) {
            out.extend(base.quads_in_graph(&graph, None));
        }
    }
    Ok(out)
}

/// Build a KO [`OntologyObject`] from the environment contract
/// (`ONTOLITH_ONTOLOGY_{TBOX,ABOX,ANNOTATION,RULE,PROVENANCE}` = graph IRIs).
/// Returns `None` when no role graph is configured (the default).
pub fn ontology_from_env() -> Option<OntologyObject> {
    let mut ontology = OntologyObject::new(
        ObjectId::from_validated("ontology:configured"),
        TimestampMs::default(),
    )
    .ok()?;
    if let Ok(value) = env::var(ONTOLOGY_TBOX_ENV) {
        ontology.tbox_graph = Some(GraphId::named(Iri::new(value)));
    }
    if let Ok(value) = env::var(ONTOLOGY_ABOX_ENV) {
        ontology.abox_graph = Some(GraphId::named(Iri::new(value)));
    }
    if let Ok(value) = env::var(ONTOLOGY_ANNOTATION_ENV) {
        ontology.annotation_graph = Some(GraphId::named(Iri::new(value)));
    }
    if let Ok(value) = env::var(ONTOLOGY_RULE_ENV) {
        ontology.rule_graph = Some(GraphId::named(Iri::new(value)));
    }
    if let Ok(value) = env::var(ONTOLOGY_PROVENANCE_ENV) {
        ontology.provenance_graph = Some(GraphId::named(Iri::new(value)));
    }
    let configured = [
        ontology.tbox_graph.as_ref(),
        ontology.abox_graph.as_ref(),
        ontology.annotation_graph.as_ref(),
        ontology.rule_graph.as_ref(),
        ontology.provenance_graph.as_ref(),
    ]
    .iter()
    .any(Option::is_some);
    configured.then_some(ontology)
}

/// [`OntologyGraphReader`] over a [`QueryReadService`]: named graphs resolve
/// through `quads_in_graph`, the default graph through `all_triples`.
pub struct QueryReadOntologyReader<'a> {
    base: &'a dyn QueryReadService,
}

impl<'a> QueryReadOntologyReader<'a> {
    pub fn new(base: &'a dyn QueryReadService) -> Self {
        Self { base }
    }
}

impl OntologyGraphReader for QueryReadOntologyReader<'_> {
    fn graph_triples(&self, graph: &GraphId) -> Result<Vec<Triple>, OntolithError> {
        match graph {
            GraphId::Named(name) => Ok(self.base.quads_in_graph(name, None)),
            GraphId::Default => self.base.all_triples(None),
        }
    }
}

/// Read-service overlay serving the base store plus a materialized inference
/// closure (P6-03). Inferred triples are visible to default-graph reads and
/// to named-graph reads (upstream tenant scoping restricts which graphs are
/// reachable, so cross-tenant data stays invisible).
pub struct ReasoningReadService {
    base: Arc<dyn QueryReadService>,
    inferred: Vec<Triple>,
}

impl ReasoningReadService {
    pub fn new(base: Arc<dyn QueryReadService>, inferred: Vec<Triple>) -> Self {
        Self { base, inferred }
    }
}

impl QueryReadService for ReasoningReadService {
    fn all_triples(&self, txn_id: Option<TxnId>) -> Result<Vec<Triple>, OntolithError> {
        let mut out = self.base.all_triples(txn_id)?;
        out.extend(self.inferred.iter().cloned());
        Ok(out)
    }

    fn by_subject(
        &self,
        subject: NodeId,
        txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        let mut out = self.base.by_subject(subject, txn_id)?;
        out.extend(
            self.inferred
                .iter()
                .filter(|t| t.subject == subject)
                .cloned(),
        );
        Ok(out)
    }

    fn by_predicate(
        &self,
        predicate: &Iri,
        txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        let mut out = self.base.by_predicate(predicate, txn_id)?;
        out.extend(
            self.inferred
                .iter()
                .filter(|t| &t.predicate == predicate)
                .cloned(),
        );
        Ok(out)
    }

    fn by_object(
        &self,
        object: &Term,
        txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        let mut out = self.base.by_object(object, txn_id)?;
        out.extend(
            self.inferred
                .iter()
                .filter(|t| &t.object == object)
                .cloned(),
        );
        Ok(out)
    }

    fn node_for_iri(&self, iri: &Iri) -> Result<Option<NodeId>, OntolithError> {
        self.base.node_for_iri(iri)
    }

    fn encode_node(&self, value: &str) -> Option<NodeId> {
        self.base.encode_node(value)
    }

    fn decode_node(&self, node_id: NodeId) -> Option<String> {
        self.base.decode_node(node_id)
    }

    fn quads_in_graph(&self, graph: &Iri, txn_id: Option<TxnId>) -> Vec<Triple> {
        let mut out = self.base.quads_in_graph(graph, txn_id);
        out.extend(self.inferred.iter().cloned());
        out
    }

    fn named_graph_names(&self, txn_id: Option<TxnId>) -> Vec<Iri> {
        self.base.named_graph_names(txn_id)
    }
}

pub fn status() -> &'static str {
    "reasoning"
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_storage::infrastructure::InMemoryDictionary;

    #[test]
    fn inference_mode_parse_roundtrip() {
        for (text, expected) in [
            ("off", InferenceMode::Off),
            ("OFF", InferenceMode::Off),
            ("disabled", InferenceMode::Off),
            ("forward", InferenceMode::ForwardChaining),
            ("forward-chaining", InferenceMode::ForwardChaining),
            ("hybrid", InferenceMode::Hybrid),
        ] {
            assert_eq!(InferenceMode::parse(text), Some(expected), "parse {text}");
            assert_eq!(
                expected.as_str(),
                InferenceMode::parse(text).unwrap().as_str()
            );
        }
        assert_eq!(InferenceMode::parse("bogus"), None);
    }

    #[test]
    fn inference_config_override_keeps_guards() {
        let base = InferenceConfig::new(InferenceMode::Off, 7, Some(250));
        let overridden = base.with_override("forward").expect("override");
        assert_eq!(overridden.mode, InferenceMode::ForwardChaining);
        assert_eq!(overridden.max_iterations, 7);
        assert_eq!(overridden.max_elapsed_ms, Some(250));
        assert!(base.with_override("nope").is_err());
    }

    #[test]
    fn default_config_is_off() {
        let cfg = InferenceConfig::default();
        assert_eq!(cfg.mode, InferenceMode::Off);
        assert!(!cfg.is_enabled());
    }

    #[test]
    fn reasoning_read_service_overlays_inferred() {
        let dict = InMemoryDictionary::new();
        let base: Arc<dyn QueryReadService> = Arc::new(NoTriplesRead);
        let inferred = vec![Triple::new(
            dict.encode_node("urn:x"),
            Iri::new("urn:p"),
            Term::Iri(Iri::new("urn:y")),
        )];
        let read = ReasoningReadService::new(base, inferred);
        let all = read.all_triples(None).expect("all");
        assert_eq!(all.len(), 1);
        let by_subject = read
            .by_subject(dict.encode_node("urn:x"), None)
            .expect("by_subject");
        assert_eq!(by_subject.len(), 1);
    }

    struct NoTriplesRead;

    impl QueryReadService for NoTriplesRead {
        fn all_triples(&self, _txn_id: Option<TxnId>) -> Result<Vec<Triple>, OntolithError> {
            Ok(Vec::new())
        }

        fn by_subject(
            &self,
            _subject: NodeId,
            _txn_id: Option<TxnId>,
        ) -> Result<Vec<Triple>, OntolithError> {
            Ok(Vec::new())
        }

        fn by_predicate(
            &self,
            _predicate: &Iri,
            _txn_id: Option<TxnId>,
        ) -> Result<Vec<Triple>, OntolithError> {
            Ok(Vec::new())
        }

        fn by_object(
            &self,
            _object: &Term,
            _txn_id: Option<TxnId>,
        ) -> Result<Vec<Triple>, OntolithError> {
            Ok(Vec::new())
        }
    }
}
