//! Query infrastructure: SPARQL parse, optimize, execute (L3 full).

mod execute;
mod hashes;
mod optimize;
mod sparql_parse;

// Keep legacy name available for external references.
#[allow(dead_code)]
mod sparql_mvp_legacy {
    // Intentionally empty shim — full engine replaces sparql_mvp.
}

use crate::application::{
    EngineUpdateWriteService, QueryExecutor, QueryPlanner, QueryReadService, QueryStatistics,
    UpdateWriteService,
};
#[cfg(test)]
use crate::domain::QueryResultSummary;
use crate::domain::{QueryKind, QueryPlan, QueryRequest, QueryResult};
use ontolith_core::domain::{Iri, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_storage::application::{
    DictionaryCodec, QuadRepository, StorageEngine, TripleRepository,
};
use ontolith_storage::infrastructure::EngineQuadRepository;
use ontolith_transaction::domain::TxnId;
use std::sync::Arc;

pub use execute::AlgebraExecutor;
pub use optimize::{CostBasedOptimizer, RuleBasedOptimizer};
pub use sparql_parse::{parse_subject_hint, plan_query};

/// [`QueryStatistics`] over a storage engine's incremental counters.
pub struct EngineQueryStatistics {
    engine: Arc<dyn StorageEngine>,
}

impl EngineQueryStatistics {
    pub fn new(engine: Arc<dyn StorageEngine>) -> Self {
        Self { engine }
    }
}

impl QueryStatistics for EngineQueryStatistics {
    fn triple_count(&self) -> u64 {
        self.engine.stats().triple_count
    }

    fn distinct_subjects(&self) -> u64 {
        self.engine.stats().distinct_subjects
    }

    fn distinct_predicates(&self) -> u64 {
        self.engine.stats().distinct_predicates
    }

    fn distinct_objects(&self) -> u64 {
        self.engine.stats().distinct_objects
    }
}

/// Storage-backed read service using SPO/POS/OSP indexes (plus optional
/// named-graph access through a [`QuadRepository`]).
pub struct InMemoryQueryReadService {
    triple_repo: Arc<dyn TripleRepository>,
    dictionary: Option<Arc<dyn DictionaryCodec>>,
    quad_repo: Option<Arc<dyn QuadRepository>>,
}

impl InMemoryQueryReadService {
    pub fn new(triple_repo: Arc<dyn TripleRepository>) -> Self {
        Self {
            triple_repo,
            dictionary: None,
            quad_repo: None,
        }
    }

    pub fn with_dictionary(
        triple_repo: Arc<dyn TripleRepository>,
        dictionary: Arc<dyn DictionaryCodec>,
    ) -> Self {
        Self {
            triple_repo,
            dictionary: Some(dictionary),
            quad_repo: None,
        }
    }

    pub fn with_quads(
        triple_repo: Arc<dyn TripleRepository>,
        dictionary: Option<Arc<dyn DictionaryCodec>>,
        quad_repo: Arc<dyn QuadRepository>,
    ) -> Self {
        Self {
            triple_repo,
            dictionary,
            quad_repo: Some(quad_repo),
        }
    }
}

impl QueryReadService for InMemoryQueryReadService {
    fn all_triples(&self, txn_id: Option<TxnId>) -> Result<Vec<Triple>, OntolithError> {
        Ok(self.triple_repo.all_in_txn(txn_id))
    }

    fn by_subject(
        &self,
        subject: NodeId,
        txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        Ok(self.triple_repo.by_subject_in_txn(subject, txn_id))
    }

    fn by_predicate(
        &self,
        predicate: &Iri,
        txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        Ok(self.triple_repo.by_predicate_in_txn(predicate, txn_id))
    }

    fn by_object(
        &self,
        object: &Term,
        txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        Ok(self.triple_repo.by_object_in_txn(object, txn_id))
    }

    fn node_for_iri(&self, iri: &Iri) -> Result<Option<NodeId>, OntolithError> {
        Ok(self
            .dictionary
            .as_ref()
            .map(|dict| dict.encode_node(iri.as_str())))
    }

    fn encode_node(&self, value: &str) -> Option<NodeId> {
        self.dictionary.as_ref().map(|dict| dict.encode_node(value))
    }

    fn decode_node(&self, node_id: NodeId) -> Option<String> {
        self.dictionary
            .as_ref()
            .and_then(|dict| dict.decode_node(node_id))
    }

    fn matching(
        &self,
        subject: Option<NodeId>,
        predicate: Option<&Iri>,
        object: Option<&Term>,
        txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        Ok(self
            .triple_repo
            .matching_in_txn(subject, predicate, object, txn_id))
    }

    fn quads_in_graph(&self, graph: &Iri, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.quad_repo
            .as_ref()
            .map(|quads| {
                quads
                    .by_graph_name_in_txn(graph, txn_id)
                    .into_iter()
                    .map(|q| q.triple)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn named_graph_names(&self, txn_id: Option<TxnId>) -> Vec<Iri> {
        self.quad_repo
            .as_ref()
            .map(|quads| {
                let mut names: Vec<Iri> = quads
                    .all_in_txn(txn_id)
                    .into_iter()
                    .filter_map(|q| q.graph_name)
                    .collect();
                names.sort_by(|a, b| a.as_str().cmp(b.as_str()));
                names.dedup();
                names
            })
            .unwrap_or_default()
    }
}

/// Full SPARQL planner.
#[derive(Debug, Default, Clone, Copy)]
pub struct SimpleQueryPlanner;

impl QueryPlanner for SimpleQueryPlanner {
    fn plan(&self, request: &QueryRequest) -> Result<QueryPlan, OntolithError> {
        plan_query(request)
    }
}

/// Executor adapter implementing [`QueryExecutor`].
pub struct ReadServiceQueryExecutor {
    inner: AlgebraExecutor,
}

impl ReadServiceQueryExecutor {
    pub fn new(read_service: Arc<dyn QueryReadService>) -> Self {
        Self {
            inner: AlgebraExecutor::new(read_service),
        }
    }
}

impl QueryExecutor for ReadServiceQueryExecutor {
    fn execute(
        &self,
        plan: &QueryPlan,
        request: &QueryRequest,
    ) -> Result<QueryResult, OntolithError> {
        self.inner.execute(plan, request)
    }
}

/// Executor that handles SPARQL Update plans (write) and delegates reads.
pub struct UpdateQueryExecutor<W> {
    inner: ReadServiceQueryExecutor,
    read: Arc<dyn QueryReadService>,
    write: W,
}

impl<W: UpdateWriteService> UpdateQueryExecutor<W> {
    pub fn new(read_service: Arc<dyn QueryReadService>, write_service: W) -> Self {
        Self {
            inner: ReadServiceQueryExecutor::new(Arc::clone(&read_service)),
            read: read_service,
            write: write_service,
        }
    }
}

impl<W: UpdateWriteService> QueryExecutor for UpdateQueryExecutor<W> {
    fn execute(
        &self,
        plan: &QueryPlan,
        request: &QueryRequest,
    ) -> Result<QueryResult, OntolithError> {
        if plan.kind == QueryKind::Update {
            execute::execute_update(plan, request, self.read.as_ref(), &self.write)
        } else {
            self.inner.execute(plan, request)
        }
    }
}

/// Build the standard L3 pipeline: parse → rule optimize → execute.
pub fn standard_pipeline(
    repo: Arc<dyn TripleRepository>,
) -> crate::application::QueryPipeline<
    SimpleQueryPlanner,
    RuleBasedOptimizer,
    ReadServiceQueryExecutor,
> {
    let read: Arc<dyn QueryReadService> = Arc::new(InMemoryQueryReadService::new(repo));
    crate::application::QueryPipeline::new(
        SimpleQueryPlanner,
        RuleBasedOptimizer,
        ReadServiceQueryExecutor::new(read),
    )
}

/// Pipeline with SPARQL Update support over a storage engine (memory or RocksDB).
pub fn update_pipeline(
    repo: Arc<dyn TripleRepository>,
    engine: Arc<dyn StorageEngine>,
    dictionary: Option<Arc<dyn DictionaryCodec>>,
) -> crate::application::QueryPipeline<
    SimpleQueryPlanner,
    CostBasedOptimizer<EngineQueryStatistics>,
    UpdateQueryExecutor<EngineUpdateWriteService>,
> {
    let quads: Arc<dyn QuadRepository> = Arc::new(EngineQuadRepository::new(Arc::clone(&engine)));
    let read: Arc<dyn QueryReadService> = Arc::new(InMemoryQueryReadService::with_quads(
        repo, dictionary, quads,
    ));
    crate::application::QueryPipeline::new(
        SimpleQueryPlanner,
        CostBasedOptimizer::new(Arc::new(EngineQueryStatistics::new(Arc::clone(&engine)))),
        UpdateQueryExecutor::new(read, EngineUpdateWriteService::new(engine)),
    )
}

/// SPARQL Update pipeline over a caller-supplied read service (P6-03: the
/// server injects a reasoning overlay read service after materialization).
pub fn update_pipeline_with_read(
    read: Arc<dyn QueryReadService>,
    engine: Arc<dyn StorageEngine>,
) -> crate::application::QueryPipeline<
    SimpleQueryPlanner,
    CostBasedOptimizer<EngineQueryStatistics>,
    UpdateQueryExecutor<EngineUpdateWriteService>,
> {
    crate::application::QueryPipeline::new(
        SimpleQueryPlanner,
        CostBasedOptimizer::new(Arc::new(EngineQueryStatistics::new(Arc::clone(&engine)))),
        UpdateQueryExecutor::new(read, EngineUpdateWriteService::new(engine)),
    )
}

/// Read-only pipeline with cost-based BGP ordering over engine statistics.
pub fn cost_pipeline(
    repo: Arc<dyn TripleRepository>,
    engine: Arc<dyn StorageEngine>,
) -> crate::application::QueryPipeline<
    SimpleQueryPlanner,
    CostBasedOptimizer<EngineQueryStatistics>,
    ReadServiceQueryExecutor,
> {
    let read: Arc<dyn QueryReadService> = Arc::new(InMemoryQueryReadService::new(repo));
    crate::application::QueryPipeline::new(
        SimpleQueryPlanner,
        CostBasedOptimizer::new(Arc::new(EngineQueryStatistics::new(engine))),
        ReadServiceQueryExecutor::new(read),
    )
}

/// Build standard L3 pipeline with a dictionary bridge for subject/IRI joins.
pub fn standard_pipeline_with_dictionary(
    repo: Arc<dyn TripleRepository>,
    dictionary: Arc<dyn DictionaryCodec>,
) -> crate::application::QueryPipeline<
    SimpleQueryPlanner,
    RuleBasedOptimizer,
    ReadServiceQueryExecutor,
> {
    let read: Arc<dyn QueryReadService> =
        Arc::new(InMemoryQueryReadService::with_dictionary(repo, dictionary));
    crate::application::QueryPipeline::new(
        SimpleQueryPlanner,
        RuleBasedOptimizer,
        ReadServiceQueryExecutor::new(read),
    )
}

pub fn status() -> &'static str {
    "infrastructure"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::QueryPipeline;
    use crate::domain::{Algebra, BoundValue, QueryRequest, TenantScope, TermPattern, TriplePattern};
    use ontolith_core::domain::{Iri, LiteralValue, NodeId};
    use ontolith_rdf::domain::{Quad, Term, Triple};
    use ontolith_storage::application::{
        DictionaryCodec, QuadRepository, StorageEngine, TripleRepository,
    };
    use ontolith_storage::infrastructure::{
        InMemoryDictionary, InMemoryQuadRepository, InMemoryStorageEngine, InMemoryTripleRepository,
    };
    use ontolith_transaction::domain::TxnId;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn seed() -> (Arc<InMemoryStorageEngine>, Arc<dyn TripleRepository>) {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        // alice knows bob
        repo.insert(
            TxnId::new(1),
            Triple {
                subject: NodeId::new(1),
                predicate: Iri::new("http://ex.org/knows"),
                object: Term::Iri(Iri::new("http://ex.org/bob")),
            },
        )
        .unwrap();
        // alice name "Alice"
        repo.insert(
            TxnId::new(1),
            Triple {
                subject: NodeId::new(1),
                predicate: Iri::new("http://ex.org/name"),
                object: Term::Literal(LiteralValue::String("Alice".into())),
            },
        )
        .unwrap();
        // bob knows carol
        repo.insert(
            TxnId::new(1),
            Triple {
                subject: NodeId::new(2),
                predicate: Iri::new("http://ex.org/knows"),
                object: Term::Iri(Iri::new("http://ex.org/carol")),
            },
        )
        .unwrap();
        // bob age 30
        repo.insert(
            TxnId::new(1),
            Triple {
                subject: NodeId::new(2),
                predicate: Iri::new("http://ex.org/age"),
                object: Term::Literal(LiteralValue::Integer(30)),
            },
        )
        .unwrap();
        engine.commit_transaction(TxnId::new(1)).unwrap();
        // encode dictionary-style ids used in SPARQL via node:N and absolute IRIs on predicates/objects
        (engine, repo)
    }

    fn pipeline(
        repo: Arc<dyn TripleRepository>,
    ) -> QueryPipeline<SimpleQueryPlanner, RuleBasedOptimizer, ReadServiceQueryExecutor> {
        standard_pipeline(repo)
    }

    /// Dictionary-backed seed: alice/bob have `<name>` literals.
    fn seed_update() -> (
        Arc<InMemoryStorageEngine>,
        Arc<InMemoryDictionary>,
        Arc<dyn TripleRepository>,
    ) {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let dict = Arc::new(InMemoryDictionary::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        let txn = TxnId::new(1);
        for (s, name) in [
            ("http://ex.org/alice", "Alice"),
            ("http://ex.org/bob", "Bob"),
        ] {
            repo.insert(
                txn,
                Triple {
                    subject: dict.encode_node(s),
                    predicate: Iri::new("http://ex.org/name"),
                    object: Term::Literal(LiteralValue::String(name.into())),
                },
            )
            .unwrap();
        }
        engine.commit_transaction(txn).unwrap();
        (engine, dict, repo)
    }

    fn update_pipeline(
        engine: Arc<InMemoryStorageEngine>,
        dict: Arc<InMemoryDictionary>,
        repo: Arc<dyn TripleRepository>,
    ) -> crate::application::QueryPipeline<
        SimpleQueryPlanner,
        CostBasedOptimizer<EngineQueryStatistics>,
        UpdateQueryExecutor<EngineUpdateWriteService>,
    > {
        crate::infrastructure::update_pipeline(repo, engine, Some(dict))
    }

    fn count_names(
        p: &crate::application::QueryPipeline<
            SimpleQueryPlanner,
            CostBasedOptimizer<EngineQueryStatistics>,
            UpdateQueryExecutor<EngineUpdateWriteService>,
        >,
    ) -> usize {
        let r = p
            .execute(&QueryRequest::new(
                "SELECT (COUNT(?n) AS ?c) WHERE { ?s <http://ex.org/name> ?n }",
            ))
            .unwrap();
        r.solutions[0]
            .get("c")
            .map(|v| match v {
                BoundValue::Literal(LiteralValue::Integer(i)) => *i as usize,
                _ => 0,
            })
            .unwrap_or(0)
    }

    #[test]
    fn update_insert_data() {
        let (engine, dict, repo) = seed_update();
        let p = update_pipeline(engine, dict, repo);
        let r = p
            .execute(&QueryRequest::new(
                "INSERT DATA { <http://ex.org/carol> <http://ex.org/name> \"Carol\" }",
            ))
            .unwrap();
        assert_eq!(r.kind, crate::domain::QueryKind::Update);
        assert_eq!(r.affected, 1);
        assert_eq!(count_names(&p), 3);
    }

    #[test]
    fn execute_planned_matches_execute() {
        let (engine, dict, repo) = seed_update();
        let p = update_pipeline(engine, dict, repo);
        let req = QueryRequest::new("SELECT ?s WHERE { ?s <http://ex.org/name> ?n }");
        let plan = p.plan(&req).expect("plan");
        let via_execute = p.execute(&req).expect("execute");
        let via_planned = p.execute_planned(&plan, &req).expect("execute_planned");
        assert_eq!(via_planned.kind, via_execute.kind);
        assert_eq!(via_planned.solutions, via_execute.solutions);
        assert_eq!(via_planned.solutions.len(), 2);
    }

    /// Minimal read overlay: base triples plus one extra virtual triple
    /// (the shape the server's reasoning overlay uses after materialization).
    struct OverlayRead {
        base: Arc<dyn QueryReadService>,
        extra: Triple,
    }

    impl QueryReadService for OverlayRead {
        fn all_triples(&self, txn_id: Option<TxnId>) -> Result<Vec<Triple>, OntolithError> {
            let mut out = self.base.all_triples(txn_id)?;
            out.push(self.extra.clone());
            Ok(out)
        }

        fn by_subject(
            &self,
            subject: NodeId,
            txn_id: Option<TxnId>,
        ) -> Result<Vec<Triple>, OntolithError> {
            let mut out = self.base.by_subject(subject, txn_id)?;
            if self.extra.subject == subject {
                out.push(self.extra.clone());
            }
            Ok(out)
        }

        fn by_predicate(
            &self,
            predicate: &Iri,
            txn_id: Option<TxnId>,
        ) -> Result<Vec<Triple>, OntolithError> {
            let mut out = self.base.by_predicate(predicate, txn_id)?;
            if &self.extra.predicate == predicate {
                out.push(self.extra.clone());
            }
            Ok(out)
        }

        fn by_object(
            &self,
            object: &Term,
            txn_id: Option<TxnId>,
        ) -> Result<Vec<Triple>, OntolithError> {
            let mut out = self.base.by_object(object, txn_id)?;
            if &self.extra.object == object {
                out.push(self.extra.clone());
            }
            Ok(out)
        }

        fn quads_in_graph(&self, graph: &Iri, txn_id: Option<TxnId>) -> Vec<Triple> {
            let mut out = self.base.quads_in_graph(graph, txn_id);
            out.push(self.extra.clone());
            out
        }
    }

    #[test]
    fn update_pipeline_with_read_serves_overlay() {
        let (engine, dict, repo) = seed_update();
        let base: Arc<dyn QueryReadService> = Arc::new(InMemoryQueryReadService::new(repo));
        let read: Arc<dyn QueryReadService> = Arc::new(OverlayRead {
            base,
            extra: Triple {
                subject: dict.encode_node("http://ex.org/carol"),
                predicate: Iri::new("http://ex.org/name"),
                object: Term::Literal(LiteralValue::String("Carol".into())),
            },
        });
        let p = crate::infrastructure::update_pipeline_with_read(read, engine);
        assert_eq!(count_names(&p), 3, "overlay triple must be visible");
    }

    #[test]
    fn update_delete_data() {
        let (engine, dict, repo) = seed_update();
        let p = update_pipeline(engine, dict, repo);
        let r = p
            .execute(&QueryRequest::new(
                "DELETE DATA { <http://ex.org/alice> <http://ex.org/name> \"Alice\" }",
            ))
            .unwrap();
        assert_eq!(r.affected, 1);
        assert_eq!(count_names(&p), 1);
    }

    #[test]
    fn update_insert_where_materializes_template() {
        let (engine, dict, repo) = seed_update();
        let p = update_pipeline(engine, dict, repo);
        let r = p
            .execute(&QueryRequest::new(
                "INSERT { ?s <http://ex.org/p> <http://ex.org/q> } WHERE { ?s <http://ex.org/name> ?n }",
            ))
            .unwrap();
        assert_eq!(r.affected, 2);
        let q = p
            .execute(&QueryRequest::new(
                "SELECT (COUNT(?s) AS ?c) WHERE { ?s <http://ex.org/p> <http://ex.org/q> }",
            ))
            .unwrap();
        assert_eq!(
            q.solutions[0].get("c"),
            Some(&BoundValue::Literal(LiteralValue::Integer(2)))
        );
    }

    #[test]
    fn update_delete_where() {
        let (engine, dict, repo) = seed_update();
        let p = update_pipeline(engine, dict, repo);
        let r = p
            .execute(&QueryRequest::new(
                "DELETE WHERE { ?s <http://ex.org/name> ?n }",
            ))
            .unwrap();
        assert_eq!(r.affected, 2);
        assert_eq!(count_names(&p), 0);
    }

    #[test]
    fn update_delete_insert_rename() {
        let (engine, dict, repo) = seed_update();
        let p = update_pipeline(engine, dict, repo);
        let r = p
            .execute(&QueryRequest::new(
                "DELETE { ?s <http://ex.org/name> ?n } INSERT { ?s <http://ex.org/renamed> ?n } WHERE { ?s <http://ex.org/name> ?n }",
            ))
            .unwrap();
        assert_eq!(r.affected, 4);
        assert_eq!(count_names(&p), 0);
        let q = p
            .execute(&QueryRequest::new(
                "SELECT (COUNT(?n) AS ?c) WHERE { ?s <http://ex.org/renamed> ?n }",
            ))
            .unwrap();
        assert_eq!(
            q.solutions[0].get("c"),
            Some(&BoundValue::Literal(LiteralValue::Integer(2)))
        );
    }

    #[test]
    fn update_data_block_rejects_variables() {
        let planner = SimpleQueryPlanner;
        let err = planner
            .plan(&QueryRequest::new(
                "INSERT DATA { ?s <http://ex.org/p> <http://ex.org/q> }",
            ))
            .expect_err("variables in DATA block rejected");
        assert!(err.message().contains("concrete"));
    }

    /// Seed a named graph `g` with `?s <name> ?n` quads.
    fn seed_named_graph(
        engine: &Arc<InMemoryStorageEngine>,
        dict: &Arc<InMemoryDictionary>,
        graph: &Iri,
        names: &[(&str, &str)],
    ) {
        let quads = InMemoryQuadRepository::new(Arc::clone(engine));
        let txn = TxnId::new(90);
        for (s, name) in names {
            quads
                .insert(
                    txn,
                    Quad::in_named_graph(
                        Triple {
                            subject: dict.encode_node(s),
                            predicate: Iri::new("http://ex.org/name"),
                            object: Term::Literal(LiteralValue::String((*name).into())),
                        },
                        graph.clone(),
                    ),
                )
                .unwrap();
        }
        engine.commit_transaction(txn).unwrap();
    }

    fn count_named_quads(engine: &Arc<InMemoryStorageEngine>, graph: &Iri) -> usize {
        engine
            .named_graph_quads()
            .into_iter()
            .filter(|q| q.graph_name.as_ref() == Some(graph))
            .count()
    }

    #[test]
    fn update_clear_default_keeps_named_graphs() {
        let (engine, dict, repo) = seed_update();
        let g = Iri::new("http://ex.org/g1");
        seed_named_graph(
            &engine,
            &dict,
            &g,
            &[
                ("http://ex.org/alice", "Alice"),
                ("http://ex.org/bob", "Bob"),
            ],
        );
        let p = update_pipeline(engine.clone(), dict, repo);
        let r = p.execute(&QueryRequest::new("CLEAR DEFAULT")).unwrap();
        assert_eq!(r.kind, crate::domain::QueryKind::Update);
        assert_eq!(r.affected, 2);
        assert_eq!(count_names(&p), 0);
        assert_eq!(count_named_quads(&engine, &g), 2);
    }

    #[test]
    fn update_clear_named_removes_all_named_graphs() {
        let (engine, dict, repo) = seed_update();
        let g1 = Iri::new("http://ex.org/g1");
        let g2 = Iri::new("http://ex.org/g2");
        seed_named_graph(
            &engine,
            &dict,
            &g1,
            &[
                ("http://ex.org/alice", "Alice"),
                ("http://ex.org/bob", "Bob"),
            ],
        );
        seed_named_graph(
            &engine,
            &dict,
            &g2,
            &[
                ("http://ex.org/carol", "Carol"),
                ("http://ex.org/dave", "Dave"),
            ],
        );
        let p = update_pipeline(engine.clone(), dict, repo);
        let r = p.execute(&QueryRequest::new("CLEAR NAMED")).unwrap();
        assert_eq!(r.affected, 4);
        assert_eq!(count_named_quads(&engine, &g1), 0);
        assert_eq!(count_named_quads(&engine, &g2), 0);
        assert_eq!(count_names(&p), 2);
    }

    #[test]
    fn update_clear_graph_removes_only_that_graph() {
        let (engine, dict, repo) = seed_update();
        let g1 = Iri::new("http://ex.org/g1");
        let g2 = Iri::new("http://ex.org/g2");
        seed_named_graph(
            &engine,
            &dict,
            &g1,
            &[
                ("http://ex.org/alice", "Alice"),
                ("http://ex.org/bob", "Bob"),
            ],
        );
        seed_named_graph(
            &engine,
            &dict,
            &g2,
            &[
                ("http://ex.org/carol", "Carol"),
                ("http://ex.org/dave", "Dave"),
            ],
        );
        let p = update_pipeline(engine.clone(), dict, repo);
        let r = p
            .execute(&QueryRequest::new("CLEAR GRAPH <http://ex.org/g1>"))
            .unwrap();
        assert_eq!(r.affected, 2);
        assert_eq!(count_named_quads(&engine, &g1), 0);
        assert_eq!(count_named_quads(&engine, &g2), 2);
        assert_eq!(count_names(&p), 2);
    }

    #[test]
    fn update_clear_all_and_drop_all() {
        let (engine, dict, repo) = seed_update();
        let g = Iri::new("http://ex.org/g1");
        seed_named_graph(
            &engine,
            &dict,
            &g,
            &[
                ("http://ex.org/alice", "Alice"),
                ("http://ex.org/bob", "Bob"),
            ],
        );
        let p = update_pipeline(engine.clone(), dict, repo);
        let r = p.execute(&QueryRequest::new("CLEAR ALL")).unwrap();
        assert_eq!(r.affected, 4);
        assert_eq!(count_names(&p), 0);
        assert_eq!(count_named_quads(&engine, &g), 0);

        // DROP GRAPH <missing> is an idempotent no-op; DROP NAMED clears quads.
        let (engine, dict, repo) = seed_update();
        let g = Iri::new("http://ex.org/g1");
        seed_named_graph(
            &engine,
            &dict,
            &g,
            &[
                ("http://ex.org/alice", "Alice"),
                ("http://ex.org/bob", "Bob"),
            ],
        );
        let p = update_pipeline(engine.clone(), dict, repo);
        let r = p
            .execute(&QueryRequest::new("DROP GRAPH <http://ex.org/missing>"))
            .unwrap();
        assert_eq!(r.affected, 0);
        assert_eq!(count_names(&p), 2);
        let r = p.execute(&QueryRequest::new("DROP NAMED")).unwrap();
        assert_eq!(r.affected, 2);
        assert_eq!(count_named_quads(&engine, &g), 0);
        assert_eq!(count_names(&p), 2);
    }

    #[test]
    fn update_load_copies_named_graph_to_default() {
        let (engine, dict, repo) = seed_update();
        let g = Iri::new("http://ex.org/src");
        seed_named_graph(
            &engine,
            &dict,
            &g,
            &[
                ("http://ex.org/carol", "Carol"),
                ("http://ex.org/dave", "Dave"),
            ],
        );
        let p = update_pipeline(engine.clone(), dict, repo);
        let r = p
            .execute(&QueryRequest::new("LOAD <http://ex.org/src>"))
            .unwrap();
        assert_eq!(r.affected, 2);
        // Default graph now holds the seed names plus the loaded copy.
        assert_eq!(count_names(&p), 4);
        assert_eq!(count_named_quads(&engine, &g), 2);
    }

    #[test]
    fn update_load_into_graph_copies_between_graphs() {
        let (engine, dict, repo) = seed_update();
        let src = Iri::new("http://ex.org/src");
        let dst = Iri::new("http://ex.org/dst");
        seed_named_graph(
            &engine,
            &dict,
            &src,
            &[
                ("http://ex.org/carol", "Carol"),
                ("http://ex.org/dave", "Dave"),
            ],
        );
        let p = update_pipeline(engine.clone(), dict, repo);
        let r = p
            .execute(&QueryRequest::new(
                "LOAD <http://ex.org/src> INTO GRAPH <http://ex.org/dst>",
            ))
            .unwrap();
        assert_eq!(r.affected, 2);
        assert_eq!(count_named_quads(&engine, &src), 2);
        assert_eq!(count_named_quads(&engine, &dst), 2);
        assert_eq!(count_names(&p), 2);
    }

    #[test]
    fn update_with_delete_insert_targets_named_graph() {
        let (engine, dict, repo) = seed_update();
        let g = Iri::new("http://ex.org/g1");
        seed_named_graph(
            &engine,
            &dict,
            &g,
            &[
                ("http://ex.org/alice", "Alice"),
                ("http://ex.org/bob", "Bob"),
            ],
        );
        let p = update_pipeline(engine.clone(), dict, repo);
        let r = p
            .execute(&QueryRequest::new(
                "WITH <http://ex.org/g1> DELETE { ?s <http://ex.org/name> ?n } INSERT { ?s <http://ex.org/renamed> ?n } WHERE { ?s <http://ex.org/name> ?n }",
            ))
            .unwrap();
        assert_eq!(r.affected, 4);
        let names = engine
            .named_graph_quads()
            .into_iter()
            .filter(|q| {
                q.graph_name.as_ref() == Some(&g)
                    && q.triple.predicate == Iri::new("http://ex.org/name")
            })
            .count();
        assert_eq!(names, 0);
        let renamed = engine
            .named_graph_quads()
            .into_iter()
            .filter(|q| {
                q.graph_name.as_ref() == Some(&g)
                    && q.triple.predicate == Iri::new("http://ex.org/renamed")
            })
            .count();
        assert_eq!(renamed, 2);
        // Default graph untouched by the WITH-scoped delete/insert.
        assert_eq!(count_names(&p), 2);
    }

    #[test]
    fn update_with_delete_where_removes_only_graph() {
        let (engine, dict, repo) = seed_update();
        let g = Iri::new("http://ex.org/g1");
        seed_named_graph(
            &engine,
            &dict,
            &g,
            &[
                ("http://ex.org/alice", "Alice"),
                ("http://ex.org/bob", "Bob"),
            ],
        );
        let p = update_pipeline(engine.clone(), dict, repo);
        let r = p
            .execute(&QueryRequest::new(
                "WITH <http://ex.org/g1> DELETE WHERE { ?s <http://ex.org/name> ?n }",
            ))
            .unwrap();
        assert_eq!(r.affected, 2);
        assert_eq!(count_named_quads(&engine, &g), 0);
        assert_eq!(count_names(&p), 2);
    }

    #[test]
    fn update_with_where_reads_target_graph() {
        let (engine, dict, repo) = seed_update();
        let g = Iri::new("http://ex.org/g1");
        seed_named_graph(
            &engine,
            &dict,
            &g,
            &[
                ("http://ex.org/alice", "Alice"),
                ("http://ex.org/bob", "Bob"),
            ],
        );
        let p = update_pipeline(engine.clone(), dict, repo);
        // The WHERE matches only quads inside the WITH graph, so exactly two
        // insertions happen (alice/bob of g), not four.
        let r = p
            .execute(&QueryRequest::new(
                "WITH <http://ex.org/g1> INSERT { ?s <http://ex.org/seen> ?n } WHERE { ?s <http://ex.org/name> ?n }",
            ))
            .unwrap();
        assert_eq!(r.affected, 2);
        let seen = engine
            .named_graph_quads()
            .into_iter()
            .filter(|q| {
                q.graph_name.as_ref() == Some(&g)
                    && q.triple.predicate == Iri::new("http://ex.org/seen")
            })
            .count();
        assert_eq!(seen, 2);
    }

    #[test]
    fn update_silent_forms_accepted() {
        let (engine, dict, repo) = seed_update();
        let g = Iri::new("http://ex.org/g1");
        seed_named_graph(
            &engine,
            &dict,
            &g,
            &[
                ("http://ex.org/alice", "Alice"),
                ("http://ex.org/bob", "Bob"),
            ],
        );
        let p = update_pipeline(engine.clone(), dict, repo);
        let r = p
            .execute(&QueryRequest::new("LOAD SILENT <http://ex.org/g1>"))
            .unwrap();
        assert_eq!(r.affected, 2);
        let r = p
            .execute(&QueryRequest::new("CLEAR SILENT GRAPH <http://ex.org/g1>"))
            .unwrap();
        assert_eq!(r.affected, 2);
        assert_eq!(count_named_quads(&engine, &g), 0);
        let r = p.execute(&QueryRequest::new("DROP SILENT ALL")).unwrap();
        // Named graph was already cleared above; only default triples remain.
        assert_eq!(r.affected, 2);
        assert_eq!(count_names(&p), 0);
    }

    #[test]
    fn cost_optimizer_orders_bgp_by_selectivity_and_binding() {
        use crate::application::QueryStatistics;

        struct FixedStats {
            total: u64,
            subjects: u64,
            predicates: u64,
            objects: u64,
        }
        impl QueryStatistics for FixedStats {
            fn triple_count(&self) -> u64 {
                self.total
            }
            fn distinct_subjects(&self) -> u64 {
                self.subjects
            }
            fn distinct_predicates(&self) -> u64 {
                self.predicates
            }
            fn distinct_objects(&self) -> u64 {
                self.objects
            }
        }
        let stats = FixedStats {
            total: 100,
            subjects: 50,
            predicates: 10,
            objects: 40,
        };
        let bound_pred = |p: &str| TriplePattern {
            subject: TermPattern::Variable("s".into()),
            predicate: TermPattern::Iri(Iri::new(p)),
            object: TermPattern::Variable("o".into()),
        };
        let patterns = vec![
            bound_pred("urn:common1"),
            TriplePattern {
                subject: TermPattern::Iri(Iri::new("urn:s")),
                predicate: TermPattern::Variable("p".into()),
                object: TermPattern::Variable("o".into()),
            },
            TriplePattern {
                subject: TermPattern::Variable("s".into()),
                predicate: TermPattern::Variable("p".into()),
                object: TermPattern::Iri(Iri::new("urn:o")),
            },
            bound_pred("urn:common2"),
        ];
        let ordered =
            match super::optimize::optimize_algebra_with_stats(Algebra::Bgp(patterns), &stats) {
                Algebra::Bgp(p) => p,
                other => panic!("expected Bgp, got {other:?}"),
            };
        let sig = |t: &TermPattern| match t {
            TermPattern::Iri(i) => i.as_str().to_string(),
            _ => "?".into(),
        };
        let signatures: Vec<_> = ordered
            .iter()
            .map(|p| {
                format!(
                    "{}:{}:{}",
                    sig(&p.subject),
                    sig(&p.predicate),
                    sig(&p.object)
                )
            })
            .collect();
        // Cheapest first, then the connecting pattern (binding propagation),
        // then the remaining selective patterns by cardinality.
        assert_eq!(
            signatures,
            vec![
                "?:urn:common1:?".to_string(),
                "?:urn:common2:?".to_string(),
                "?:?:urn:o".to_string(),
                "urn:s:?:?".to_string(),
            ]
        );
    }

    #[test]
    fn engine_statistics_reflect_seed() {
        let (engine, _repo) = seed();
        let stats = EngineQueryStatistics::new(engine.clone());
        assert_eq!(stats.triple_count(), 4);
        assert_eq!(stats.distinct_predicates(), 3);
        let s = engine.stats();
        assert_eq!(stats.distinct_subjects(), s.distinct_subjects);
        assert_eq!(stats.distinct_objects(), s.distinct_objects);
    }

    #[test]
    fn cost_pipeline_matches_standard_results() {
        let (engine, repo) = seed();
        let p_std = standard_pipeline(repo.clone());
        let p_cost = cost_pipeline(repo, engine);
        for q in [
            "SELECT * WHERE { ?s ?p ?o }",
            "SELECT ?s WHERE { ?s <http://ex.org/knows> ?o . ?s <http://ex.org/name> ?n }",
        ] {
            let a = p_std.execute(&QueryRequest::new(q)).unwrap();
            let b = p_cost.execute(&QueryRequest::new(q)).unwrap();
            assert_eq!(a.solutions.len(), b.solutions.len());
        }
        let plan = p_cost
            .plan(&QueryRequest::new("SELECT * WHERE { ?s ?p ?o }"))
            .unwrap();
        assert!(
            plan.logical_steps
                .iter()
                .any(|s| s.starts_with("optimize(cost)")),
            "cost optimizer step missing"
        );
    }

    #[test]
    fn explain_includes_cost_estimates() {
        let (engine, repo) = seed();
        let p_cost = cost_pipeline(repo, engine);
        let explain = p_cost
            .explain(&QueryRequest::new(
                "SELECT * WHERE { ?s <http://ex.org/knows> ?o }",
            ))
            .unwrap();
        assert_eq!(explain.estimated_rows, Some(3));
        assert_eq!(explain.pattern_costs.len(), 1);
        let cost = &explain.pattern_costs[0];
        assert_eq!(cost.pattern, "?s <http://ex.org/knows> ?o");
        assert_eq!(cost.estimated_rows, 3);
        assert!(cost.selectivity > 0.0 && cost.selectivity <= 1.0);

        // A query without statistics keeps the fields empty.
        let (_e, repo) = seed();
        let p_std = standard_pipeline(repo);
        let explain = p_std
            .explain(&QueryRequest::new("SELECT * WHERE { ?s ?p ?o }"))
            .unwrap();
        assert_eq!(explain.estimated_rows, None);
        assert!(explain.pattern_costs.is_empty());
    }

    #[test]
    fn select_star_returns_all_triples_as_solutions() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new("SELECT * WHERE { ?s ?p ?o }"))
            .unwrap();
        assert_eq!(result.solutions.len(), 4);
        assert!(result.variables.contains(&"s".into()));
        assert!(result.variables.contains(&"p".into()));
        assert!(result.variables.contains(&"o".into()));
    }

    #[test]
    fn select_by_predicate_uses_pos() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?s ?o WHERE { ?s <http://ex.org/knows> ?o }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 2);
        let explain = p
            .explain(&QueryRequest::new(
                "SELECT ?s ?o WHERE { ?s <http://ex.org/knows> ?o }",
            ))
            .unwrap();
        assert!(
            explain
                .physical_steps
                .iter()
                .any(|s| s.contains("index_pos") || s.contains("bgp"))
        );
    }

    #[test]
    fn join_two_patterns() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        // node:1 knows ?o . node:1 name ?n
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?o ?n WHERE {
                    node:1 <http://ex.org/knows> ?o .
                    node:1 <http://ex.org/name> ?n
                }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("n"),
            Some(&BoundValue::Literal(LiteralValue::String("Alice".into())))
        );
    }

    #[test]
    fn optional_left_join() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?s ?age WHERE {
                    ?s <http://ex.org/knows> ?o .
                    OPTIONAL { ?s <http://ex.org/age> ?age }
                }"#,
            ))
            .unwrap();
        // two knows triples; only bob(node:2) has age
        assert_eq!(result.solutions.len(), 2);
        let with_age = result
            .solutions
            .iter()
            .filter(|s| s.get("age").is_some())
            .count();
        assert_eq!(with_age, 1);
    }

    #[test]
    fn filter_bound_and_compare() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?s ?age WHERE {
                    ?s <http://ex.org/age> ?age .
                    FILTER(?age >= 30)
                }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
    }

    #[test]
    fn union_combines_branches() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?x WHERE {
                    { node:1 <http://ex.org/name> ?x }
                    UNION
                    { node:2 <http://ex.org/age> ?x }
                }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 2);
    }

    #[test]
    fn bind_extends_solution() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?s ?flag WHERE {
                    ?s <http://ex.org/name> ?n .
                    BIND(BOUND(?n) AS ?flag)
                }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("flag"),
            Some(&BoundValue::Literal(LiteralValue::Boolean(true)))
        );
    }

    #[test]
    fn values_clause() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?s ?o WHERE {
                    VALUES ?s { node:1 }
                    ?s <http://ex.org/knows> ?o
                }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
    }

    #[test]
    fn ask_true_false() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let yes = p
            .execute(&QueryRequest::new(
                "ASK WHERE { ?s <http://ex.org/knows> ?o }",
            ))
            .unwrap();
        assert_eq!(yes.boolean, Some(true));
        let no = p
            .execute(&QueryRequest::new(
                "ASK WHERE { ?s <http://ex.org/missing> ?o }",
            ))
            .unwrap();
        assert_eq!(no.boolean, Some(false));
    }

    #[test]
    fn construct_builds_triples() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"CONSTRUCT { ?s <http://ex.org/copy> ?o }
                   WHERE { ?s <http://ex.org/knows> ?o }"#,
            ))
            .unwrap();
        assert_eq!(result.construct_triples.len(), 2);
    }

    #[test]
    fn distinct_and_limit_offset() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT DISTINCT ?p WHERE { ?s ?p ?o } ORDER BY ?p LIMIT 1 OFFSET 0",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
    }

    #[test]
    fn explain_contains_optimize_step() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let explain = p
            .explain(&QueryRequest::new(
                "SELECT * WHERE { ?s <http://ex.org/knows> ?o }",
            ))
            .unwrap();
        assert!(
            explain
                .logical_steps
                .iter()
                .any(|s| s.starts_with("optimize:"))
        );
        assert!(!explain.algebra_summary.is_empty());
    }

    #[test]
    fn timeout_zero() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new("SELECT * WHERE { ?s ?p ?o }").with_timeout(0))
            .unwrap();
        assert!(result.timed_out);
    }

    #[test]
    fn cancel_flag() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let flag = Arc::new(AtomicBool::new(true));
        let result = p
            .execute(&QueryRequest::new("SELECT * WHERE { ?s ?p ?o }").with_cancel(flag))
            .unwrap();
        assert!(result.cancelled);
    }

    /// Seed `n` triples with the same predicate for join-heavy preemption tests.
    fn seed_many(n: u64) -> (Arc<InMemoryStorageEngine>, Arc<dyn TripleRepository>) {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        let txn = TxnId::new(1);
        for i in 0..n {
            repo.insert(
                txn,
                Triple {
                    subject: NodeId::new(i),
                    predicate: Iri::new("http://ex.org/p"),
                    object: Term::Iri(Iri::new(format!("http://ex.org/o{i}"))),
                },
            )
            .unwrap();
        }
        engine.commit_transaction(txn).unwrap();
        (engine, repo)
    }

    #[test]
    fn preemption_token_deadline_and_cancel() {
        use crate::domain::{PreemptionReason, PreemptionToken};

        let token = PreemptionToken::new(Some(1));
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(token.reason(), Some(PreemptionReason::Timeout));
        assert!(token.is_preempted());

        let token = PreemptionToken::new(None);
        assert_eq!(token.reason(), None);
        assert!(token.remaining().is_none());
        token.preempt();
        assert_eq!(token.reason(), Some(PreemptionReason::Cancelled));
    }

    #[test]
    fn deadline_preempts_join_query() {
        let (_e, repo) = seed_many(2000);
        let p = pipeline(repo);
        let result = p
            .execute(
                &QueryRequest::new("SELECT * WHERE { ?s ?p ?o . ?s2 ?p2 ?o2 }").with_timeout(1),
            )
            .unwrap();
        assert!(result.timed_out, "expected preemption by deadline");
        assert!(result.solutions.is_empty());
    }

    #[test]
    fn async_cancel_preempts_join_query() {
        use crate::domain::PreemptionToken;

        let (_e, repo) = seed_many(2000);
        let p = pipeline(repo);
        let token = PreemptionToken::new(None);
        let flag = token.cancel_flag();
        let thread_flag = Arc::clone(&flag);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(2));
            thread_flag.store(true, Ordering::Relaxed);
        });
        let result = p
            .execute(
                &QueryRequest::new("SELECT * WHERE { ?s ?p ?o . ?s2 ?p2 ?o2 }")
                    .with_cancel(Arc::clone(&flag)),
            )
            .unwrap();
        handle.join().unwrap();
        assert!(result.cancelled, "expected async preemption");
    }

    #[test]
    fn update_preemption_returns_timed_out_without_writes() {
        let (engine, dict, repo) = {
            let engine = Arc::new(InMemoryStorageEngine::new());
            let dict = Arc::new(InMemoryDictionary::new());
            let repo: Arc<dyn TripleRepository> =
                Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
            let txn = TxnId::new(1);
            for i in 0..2000u64 {
                repo.insert(
                    txn,
                    Triple {
                        subject: dict.encode_node(&format!("http://ex.org/s{i}")),
                        predicate: Iri::new("http://ex.org/name"),
                        object: Term::Literal(LiteralValue::String(format!("n{i}"))),
                    },
                )
                .unwrap();
            }
            engine.commit_transaction(txn).unwrap();
            (engine, dict, repo)
        };
        let p = update_pipeline(engine, dict, repo);
        let result = p
            .execute(
                &QueryRequest::new(
                    "INSERT { ?s <http://ex.org/q> ?n } WHERE { ?s <http://ex.org/name> ?n }",
                )
                .with_timeout(1),
            )
            .unwrap();
        assert!(result.timed_out, "expected update preemption");
        assert_eq!(result.affected, 0);
    }

    #[test]
    fn prefix_expansion() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"PREFIX ex: <http://ex.org/>
                   SELECT ?s ?o WHERE { ?s ex:knows ?o }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 2);
    }

    #[test]
    fn legacy_subject_hint_still_works() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT * WHERE { ?s ?p ?o } # subject=1",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 2); // alice has 2 triples
    }

    #[test]
    fn txn_visibility() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        let p = pipeline(Arc::clone(&repo));
        let txn = TxnId::new(9);
        repo.insert(
            txn,
            Triple {
                subject: NodeId::new(99),
                predicate: Iri::new("http://ex.org/p"),
                object: Term::Iri(Iri::new("http://ex.org/o")),
            },
        )
        .unwrap();
        let outside = p
            .execute(&QueryRequest::new("SELECT * WHERE { ?s ?p ?o }"))
            .unwrap();
        assert_eq!(outside.solutions.len(), 0);
        let inside = p
            .execute(&QueryRequest::new("SELECT * WHERE { ?s ?p ?o }").with_txn(txn))
            .unwrap();
        assert_eq!(inside.solutions.len(), 1);
    }

    #[test]
    fn summary_compat() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let summary: QueryResultSummary = p
            .execute_summary(&QueryRequest::new(
                "SELECT * WHERE { ?s <http://ex.org/knows> ?o }",
            ))
            .unwrap();
        assert_eq!(summary.row_count, 2);
    }

    #[test]
    fn algebra_binds_node_subject() {
        let planner = SimpleQueryPlanner;
        let plan = planner
            .plan(&QueryRequest::new("SELECT * WHERE { node:1 ?p ?o }"))
            .unwrap();
        // after project wrapper
        fn find_bgp(a: &Algebra) -> bool {
            match a {
                Algebra::Bgp(p) => matches!(p[0].subject, TermPattern::Node(_)),
                Algebra::Project { input, .. }
                | Algebra::Slice { input, .. }
                | Algebra::Distinct { input }
                | Algebra::Filter { input, .. } => find_bgp(input),
                Algebra::Join { left, .. } => find_bgp(left),
                _ => false,
            }
        }
        assert!(find_bgp(&plan.algebra));
    }

    #[test]
    fn empty_query_rejected() {
        let planner = SimpleQueryPlanner;
        let err = planner.plan(&QueryRequest::new("   ")).expect_err("empty");
        assert!(matches!(err, OntolithError::InvalidArgument(_)));
    }

    #[test]
    fn unsupported_update() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let err = p
            .execute(&QueryRequest::new(
                "INSERT DATA { <http://ex.org/a> <http://ex.org/b> <http://ex.org/c> }",
            ))
            .expect_err("update");
        assert!(matches!(err, OntolithError::Unsupported(_)));
    }

    #[test]
    fn aggregate_count_without_group_by() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT (COUNT(?s) AS ?c) WHERE { ?s ?p ?o }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(result.variables, vec!["c".to_string()]);
        assert_eq!(
            result.solutions[0].get("c"),
            Some(&BoundValue::Literal(LiteralValue::Integer(4)))
        );
    }

    #[test]
    fn aggregate_count_star_without_group_by() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("c"),
            Some(&BoundValue::Literal(LiteralValue::Integer(4)))
        );
    }

    #[test]
    fn aggregate_mixed_projection_without_group_by_rejected() {
        let planner = SimpleQueryPlanner;
        let err = planner
            .plan(&QueryRequest::new(
                "SELECT ?s (COUNT(?s) AS ?c) WHERE { ?s ?p ?o }",
            ))
            .expect_err("mixed projection requires group by");
        assert!(matches!(err, OntolithError::Failed(_)));
        assert!(err.message().contains("GROUP BY"));
    }

    /// Fresh seed: <n1> <age> 30, <n2> <age> 20, <n3> <age> 30, <n3> <age> 10.
    fn seed_ages() -> (Arc<InMemoryStorageEngine>, Arc<dyn TripleRepository>) {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        repo.insert(
            TxnId::new(2),
            Triple {
                subject: NodeId::new(1),
                predicate: Iri::new("http://ex.org/age"),
                object: Term::Literal(LiteralValue::Integer(30)),
            },
        )
        .unwrap();
        repo.insert(
            TxnId::new(2),
            Triple {
                subject: NodeId::new(2),
                predicate: Iri::new("http://ex.org/age"),
                object: Term::Literal(LiteralValue::Integer(20)),
            },
        )
        .unwrap();
        repo.insert(
            TxnId::new(2),
            Triple {
                subject: NodeId::new(3),
                predicate: Iri::new("http://ex.org/age"),
                object: Term::Literal(LiteralValue::Integer(30)),
            },
        )
        .unwrap();
        repo.insert(
            TxnId::new(2),
            Triple {
                subject: NodeId::new(3),
                predicate: Iri::new("http://ex.org/age"),
                object: Term::Literal(LiteralValue::Integer(10)),
            },
        )
        .unwrap();
        engine.commit_transaction(TxnId::new(2)).unwrap();
        (engine, repo)
    }

    #[test]
    fn aggregate_group_by_count() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?s (COUNT(?p) AS ?c) WHERE { ?s ?p ?o } GROUP BY ?s",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 2);
        for s in &result.solutions {
            assert_eq!(
                s.get("c"),
                Some(&BoundValue::Literal(LiteralValue::Integer(2)))
            );
        }
    }

    #[test]
    fn aggregate_sum_avg_min_max() {
        let (_e, repo) = seed_ages();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT (SUM(?age) AS ?sum) (AVG(?age) AS ?avg) (MIN(?age) AS ?min) (MAX(?age) AS ?max) WHERE { ?s <http://ex.org/age> ?age }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        let row = &result.solutions[0];
        assert_eq!(
            row.get("sum"),
            Some(&BoundValue::Literal(LiteralValue::Integer(90)))
        );
        assert_eq!(
            row.get("min"),
            Some(&BoundValue::Literal(LiteralValue::Integer(10)))
        );
        assert_eq!(
            row.get("max"),
            Some(&BoundValue::Literal(LiteralValue::Integer(30)))
        );
        let BoundValue::Literal(LiteralValue::Decimal(avg)) = row.get("avg").unwrap() else {
            panic!("expected decimal avg");
        };
        assert!((avg - 22.5).abs() < 1e-9);
    }

    #[test]
    fn aggregate_count_distinct() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT (COUNT(DISTINCT ?p) AS ?c) WHERE { ?s ?p ?o }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("c"),
            Some(&BoundValue::Literal(LiteralValue::Integer(3)))
        );
    }

    #[test]
    fn aggregate_having() {
        let (_e, repo) = seed_ages();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?s (SUM(?age) AS ?sum) WHERE { ?s <http://ex.org/age> ?age } GROUP BY ?s HAVING (SUM(?age) > 25)",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 2);
        let mut sums: Vec<_> = result
            .solutions
            .iter()
            .map(|s| s.get("sum").unwrap().clone())
            .collect();
        sums.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        assert_eq!(
            sums,
            vec![
                BoundValue::Literal(LiteralValue::Integer(30)),
                BoundValue::Literal(LiteralValue::Integer(40)),
            ]
        );
    }

    #[test]
    fn aggregate_having_by_alias() {
        let (_e, repo) = seed_ages();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?s (COUNT(?age) AS ?c) WHERE { ?s <http://ex.org/age> ?age } GROUP BY ?s HAVING (?c >= 2)",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("c"),
            Some(&BoundValue::Literal(LiteralValue::Integer(2)))
        );
    }

    #[test]
    fn aggregate_expression_argument_avg_if_coalesce() {
        // agg-err-02 style: a non-numeric value is normalized via
        // IF(isNumeric(...), ..., COALESCE(xsd:double(...), 0)).
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT (AVG(IF(isNumeric(?o), ?o, COALESCE(xsd:double(?o),0))) AS ?avg) WHERE { VALUES ?o { 1 \"not a number\" 3 } }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        let BoundValue::Literal(LiteralValue::Decimal(avg)) = result.solutions[0].get("avg").unwrap() else {
            panic!("expected decimal avg");
        };
        // (1 + 0 + 3) / 3 = 4/3 -> 1.3333333333333333
        assert!((avg - 4.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn aggregate_sum_decimal_exact() {
        // 1.0 + 2.2 + 3.5 + 2.2 + 2.2 must be exactly 11.1 (agg-sum-01), not
        // the f64-accumulated 11.100000000000001.
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT (SUM(?o) AS ?sum) WHERE { VALUES ?o { 1.0 2.2 3.5 2.2 2.2 } }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("sum"),
            Some(&BoundValue::Literal(LiteralValue::Decimal(11.1)))
        );
    }

    #[test]
    fn aggregate_avg_empty_group_is_zero() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT (AVG(?o) AS ?avg) WHERE { ?s <http://ex.org/nope> ?o }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("avg"),
            Some(&BoundValue::Literal(LiteralValue::Integer(0)))
        );
    }

    #[test]
    fn aggregate_avg_error_propagates() {
        // A non-numeric value makes the whole AVG an error (agg-err-01).
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT (AVG(?o) AS ?avg) WHERE { VALUES ?o { 1 \"x\" 3 } }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert!(result.solutions[0].get("avg").is_none());
    }

    #[test]
    fn aggregate_group_concat_plain_and_distinct() {
        // GROUP_CONCAT always yields a simple literal; DISTINCT dedupes.
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT (GROUP_CONCAT(DISTINCT ?o) AS ?g) WHERE {
                       VALUES ?o { "1" "2" "1" }
                   }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        match result.solutions[0].get("g") {
            Some(BoundValue::Literal(LiteralValue::String(v))) => {
                assert!(v == "1 2" || v == "2 1", "got {v:?}");
            }
            other => panic!("expected plain string, got {other:?}"),
        }
    }

    #[test]
    fn aggregate_group_concat_lang_dropped() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT (GROUP_CONCAT(?o) AS ?g) WHERE {
                       VALUES ?o { "1"@en "2"@en }
                   }"#,
            ))
            .unwrap();
        assert_eq!(
            result.solutions[0].get("g"),
            Some(&BoundValue::Literal(LiteralValue::String("1 2".to_owned())))
        );
    }

    #[test]
    fn aggregate_having_multiple_constraints() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?s WHERE { VALUES ?s { 1 1 2 } } GROUP BY ?s HAVING (COUNT(*) > 1) (COUNT(*) < 3)",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("s"),
            Some(&BoundValue::Literal(LiteralValue::Integer(1)))
        );
    }

    #[test]
    fn aggregate_group_by_expr_alias() {
        let (_e, repo) = seed_ages();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?bucket (COUNT(?s) AS ?c) WHERE { ?s <http://ex.org/age> ?age } GROUP BY ((?age >= 20) AS ?bucket)",
            ))
            .unwrap();
        // age>=20 buckets: true(30,20,30) / false(10)
        assert_eq!(result.solutions.len(), 2);
        assert!(result.variables.contains(&"c".to_string()));
    }

    #[test]
    fn aggregate_in_subquery() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?s (COUNT(?p) AS ?c) WHERE {
                    {
                        SELECT ?s ?p WHERE { ?s ?p ?o }
                    }
                } GROUP BY ?s"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 2);
        for s in &result.solutions {
            assert_eq!(
                s.get("c"),
                Some(&BoundValue::Literal(LiteralValue::Integer(2)))
            );
        }
    }

    #[test]
    fn subquery_select_with_limit() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?s WHERE {
                    {
                        SELECT ?s WHERE { ?s ?p ?o }
                        LIMIT 1
                    }
                }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert!(result.variables.contains(&"s".to_string()));
        assert!(result.solutions[0].get("s").is_some());
    }

    #[test]
    fn property_path_sequence_two_predicates() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let dict = Arc::new(InMemoryDictionary::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));

        let alice = dict.encode_node("http://ex.org/alice");
        let bob = dict.encode_node("http://ex.org/bob");

        let txn = TxnId::new(1);
        repo.insert(
            txn,
            Triple {
                subject: alice,
                predicate: Iri::new("http://ex.org/knows"),
                object: Term::Iri(Iri::new("http://ex.org/bob")),
            },
        )
        .unwrap();
        repo.insert(
            txn,
            Triple {
                subject: bob,
                predicate: Iri::new("http://ex.org/age"),
                object: Term::Literal(LiteralValue::Integer(30)),
            },
        )
        .unwrap();
        engine.commit_transaction(txn).unwrap();

        let p = standard_pipeline_with_dictionary(repo, dict);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?s ?age WHERE { ?s <http://ex.org/knows>/<http://ex.org/age> ?age }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("age"),
            Some(&BoundValue::Literal(LiteralValue::Integer(30)))
        );
    }

    #[test]
    fn property_path_one_or_more_plus() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let dict = Arc::new(InMemoryDictionary::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));

        let alice = dict.encode_node("http://ex.org/alice");
        let bob = dict.encode_node("http://ex.org/bob");
        let _carol = dict.encode_node("http://ex.org/carol");

        let txn = TxnId::new(2);
        repo.insert(
            txn,
            Triple {
                subject: alice,
                predicate: Iri::new("http://ex.org/knows"),
                object: Term::Iri(Iri::new("http://ex.org/bob")),
            },
        )
        .unwrap();
        repo.insert(
            txn,
            Triple {
                subject: bob,
                predicate: Iri::new("http://ex.org/knows"),
                object: Term::Iri(Iri::new("http://ex.org/carol")),
            },
        )
        .unwrap();
        engine.commit_transaction(txn).unwrap();

        let p = standard_pipeline_with_dictionary(repo, dict);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?o WHERE { <http://ex.org/alice> <http://ex.org/knows>+ ?o }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 2);
    }

    #[test]
    fn property_path_zero_or_more_star_includes_self() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let dict = Arc::new(InMemoryDictionary::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));

        let alice = dict.encode_node("http://ex.org/alice");
        let bob = dict.encode_node("http://ex.org/bob");

        let txn = TxnId::new(3);
        repo.insert(
            txn,
            Triple {
                subject: alice,
                predicate: Iri::new("http://ex.org/knows"),
                object: Term::Iri(Iri::new("http://ex.org/bob")),
            },
        )
        .unwrap();
        engine.commit_transaction(txn).unwrap();

        let p = standard_pipeline_with_dictionary(repo, dict);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?o WHERE { <http://ex.org/alice> <http://ex.org/knows>* ?o }",
            ))
            .unwrap();

        assert_eq!(result.solutions.len(), 2);
        assert!(
            result
                .solutions
                .iter()
                .any(|s| s.get("o") == Some(&BoundValue::Node(alice)))
        );
        assert!(
            result
                .solutions
                .iter()
                .any(|s| s.get("o") == Some(&BoundValue::Node(bob)))
        );
    }

    #[test]
    fn property_path_alternative_or() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let dict = Arc::new(InMemoryDictionary::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));

        let alice = dict.encode_node("http://ex.org/alice");

        let txn = TxnId::new(4);
        repo.insert(
            txn,
            Triple {
                subject: alice,
                predicate: Iri::new("http://ex.org/name"),
                object: Term::Literal(LiteralValue::String("Alice".into())),
            },
        )
        .unwrap();
        repo.insert(
            txn,
            Triple {
                subject: alice,
                predicate: Iri::new("http://ex.org/nick"),
                object: Term::Literal(LiteralValue::String("A".into())),
            },
        )
        .unwrap();
        engine.commit_transaction(txn).unwrap();

        let p = standard_pipeline_with_dictionary(repo, dict);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?v WHERE { <http://ex.org/alice> <http://ex.org/name>|<http://ex.org/nick> ?v }",
            ))
            .unwrap();

        assert_eq!(result.solutions.len(), 2);
    }

    #[test]
    fn property_path_inverse_predicate() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let dict = Arc::new(InMemoryDictionary::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));

        let alice = dict.encode_node("http://ex.org/alice");

        let txn = TxnId::new(5);
        repo.insert(
            txn,
            Triple {
                subject: alice,
                predicate: Iri::new("http://ex.org/knows"),
                object: Term::Iri(Iri::new("http://ex.org/bob")),
            },
        )
        .unwrap();
        engine.commit_transaction(txn).unwrap();

        let p = standard_pipeline_with_dictionary(repo, dict);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?s WHERE { <http://ex.org/bob> ^<http://ex.org/knows> ?s }",
            ))
            .unwrap();

        assert_eq!(result.solutions.len(), 1);
        assert_eq!(result.solutions[0].get("s"), Some(&BoundValue::Node(alice)));
    }

    #[test]
    fn property_path_zero_or_one_matches_self_and_one_step() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let dict = Arc::new(InMemoryDictionary::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));

        let alice = dict.encode_node("http://ex.org/alice");
        let bob = dict.encode_node("http://ex.org/bob");
        let carol = dict.encode_node("http://ex.org/carol");

        let txn = TxnId::new(30);
        repo.insert(
            txn,
            Triple {
                subject: alice,
                predicate: Iri::new("http://ex.org/knows"),
                object: Term::Iri(Iri::new("http://ex.org/bob")),
            },
        )
        .unwrap();
        repo.insert(
            txn,
            Triple {
                subject: bob,
                predicate: Iri::new("http://ex.org/knows"),
                object: Term::Iri(Iri::new("http://ex.org/carol")),
            },
        )
        .unwrap();
        engine.commit_transaction(txn).unwrap();

        let p = standard_pipeline_with_dictionary(repo, dict);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?s ?o WHERE { ?s <http://ex.org/knows>? ?o }",
            ))
            .unwrap();

        // a -> {a, b}, b -> {b, c}, c -> {c}
        assert_eq!(result.solutions.len(), 5);
        let pairs: Vec<(BoundValue, BoundValue)> = result
            .solutions
            .iter()
            .map(|s| (s.get("s").unwrap().clone(), s.get("o").unwrap().clone()))
            .collect();
        assert!(pairs.contains(&(BoundValue::Node(alice), BoundValue::Node(alice))));
        assert!(pairs.contains(&(BoundValue::Node(alice), BoundValue::Node(bob))));
        assert!(pairs.contains(&(BoundValue::Node(bob), BoundValue::Node(carol))));
        assert!(pairs.contains(&(BoundValue::Node(carol), BoundValue::Node(carol))));
    }

    #[test]
    fn whitespace_before_question_mark_is_a_variable_not_modifier() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let dict = Arc::new(InMemoryDictionary::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));

        let alice = dict.encode_node("http://ex.org/alice");
        let txn = TxnId::new(31);
        repo.insert(
            txn,
            Triple {
                subject: alice,
                predicate: Iri::new("http://ex.org/name"),
                object: Term::Literal(LiteralValue::String("Alice".into())),
            },
        )
        .unwrap();
        engine.commit_transaction(txn).unwrap();

        let p = standard_pipeline_with_dictionary(repo, dict);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?s ?o WHERE { ?s <http://ex.org/name> ?o }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("o"),
            Some(&BoundValue::Literal(LiteralValue::String("Alice".into())))
        );
    }

    #[test]
    fn negation_exists_and_minus() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        // EXISTS: alice knows bob and has a name; bob knows carol but has none.
        let r = p
            .execute(&QueryRequest::new(
                "SELECT ?s WHERE { ?s <http://ex.org/knows> ?b FILTER EXISTS { ?s <http://ex.org/name> ?n } }",
            ))
            .unwrap();
        assert_eq!(r.solutions.len(), 1);
        assert_eq!(
            r.solutions[0].get("s"),
            Some(&BoundValue::Node(NodeId::new(1)))
        );
        // NOT EXISTS: bob has no name.
        let r = p
            .execute(&QueryRequest::new(
                "SELECT ?s WHERE { ?s <http://ex.org/knows> ?b FILTER NOT EXISTS { ?s <http://ex.org/name> ?n } }",
            ))
            .unwrap();
        assert_eq!(r.solutions.len(), 1);
        assert_eq!(
            r.solutions[0].get("s"),
            Some(&BoundValue::Node(NodeId::new(2)))
        );
        // MINUS: subjects who know someone, minus those with a name.
        let r = p
            .execute(&QueryRequest::new(
                "SELECT ?s WHERE { ?s <http://ex.org/knows> ?b MINUS { ?s <http://ex.org/name> ?n } }",
            ))
            .unwrap();
        assert_eq!(r.solutions.len(), 1);
        assert_eq!(
            r.solutions[0].get("s"),
            Some(&BoundValue::Node(NodeId::new(2)))
        );
    }

    #[test]
    fn function_string_operators() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let r = p
            .execute(&QueryRequest::new(
                "SELECT (UCASE(STR(?n)) AS ?u) WHERE { node:1 <http://ex.org/name> ?n }",
            ))
            .unwrap();
        assert_eq!(
            r.solutions[0].get("u"),
            Some(&BoundValue::Literal(LiteralValue::String("ALICE".into())))
        );
        let r = p
            .execute(&QueryRequest::new(
                "SELECT (LCASE(\"AbC\") AS ?l) (STRLEN(\"abcd\") AS ?n) (CONCAT(\"ab\", \"cd\") AS ?c) WHERE { ?s ?p ?o } LIMIT 1",
            ))
            .unwrap();
        assert_eq!(
            r.solutions[0].get("l"),
            Some(&BoundValue::Literal(LiteralValue::String("abc".into())))
        );
        assert_eq!(
            r.solutions[0].get("n"),
            Some(&BoundValue::Literal(LiteralValue::Integer(4)))
        );
        assert_eq!(
            r.solutions[0].get("c"),
            Some(&BoundValue::Literal(LiteralValue::String("abcd".into())))
        );
        let r = p
            .execute(&QueryRequest::new(
                "SELECT ?n WHERE { node:1 <http://ex.org/name> ?n FILTER(CONTAINS(?n, \"li\")) }",
            ))
            .unwrap();
        assert_eq!(r.solutions.len(), 1);
    }

    #[test]
    fn function_numeric_conditional_and_list() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let r = p
            .execute(&QueryRequest::new(
                "SELECT (ABS(-5) AS ?a) (CEIL(1.2) AS ?c) (ROUND(2.5) AS ?r) (IF(1 = 1, 7, 8) AS ?i) (1 IN (1, 2) AS ?in) (2 NOT IN (1, 2) AS ?nin) WHERE { ?s ?p ?o } LIMIT 1",
            ))
            .unwrap();
        assert_eq!(
            r.solutions[0].get("a"),
            Some(&BoundValue::Literal(LiteralValue::Integer(5)))
        );
        assert_eq!(
            r.solutions[0].get("c"),
            Some(&BoundValue::Literal(LiteralValue::Decimal(2.0)))
        );
        assert_eq!(
            r.solutions[0].get("i"),
            Some(&BoundValue::Literal(LiteralValue::Integer(7)))
        );
        assert_eq!(
            r.solutions[0].get("in"),
            Some(&BoundValue::Literal(LiteralValue::Boolean(true)))
        );
        assert_eq!(
            r.solutions[0].get("nin"),
            Some(&BoundValue::Literal(LiteralValue::Boolean(false)))
        );
    }

    #[test]
    fn literal_model_lang_cast_and_value_equality() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        // Language tags survive data round-trip; UCASE preserves them; CAST
        // and cross-type numeric equality follow SPARQL semantics.
        let r = p
            .execute(&QueryRequest::new(
                "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
                 SELECT (UCASE(\"bar\"@en) AS ?u) (STRLANG(\"x\", \"fr\") AS ?sl)
                        (xsd:decimal(\"+33.3300\") AS ?d) (1 = 1.0 AS ?eq)
                        (\"abc\"^^xsd:string = \"abc\" AS ?sq)
                 WHERE { ?s ?p ?o } LIMIT 1",
            ))
            .unwrap();
        assert_eq!(
            r.solutions[0].get("u"),
            Some(&BoundValue::Literal(LiteralValue::Lang {
                value: "BAR".into(),
                lang: ontolith_core::domain::LanguageTag::parse("en").unwrap(),
            }))
        );
        assert_eq!(
            r.solutions[0].get("sl"),
            Some(&BoundValue::Literal(LiteralValue::Lang {
                value: "x".into(),
                lang: ontolith_core::domain::LanguageTag::parse("fr").unwrap(),
            }))
        );
        assert_eq!(
            r.solutions[0].get("d"),
            Some(&BoundValue::Literal(LiteralValue::Decimal(33.33)))
        );
        assert_eq!(
            r.solutions[0].get("eq"),
            Some(&BoundValue::Literal(LiteralValue::Boolean(true)))
        );
        assert_eq!(
            r.solutions[0].get("sq"),
            Some(&BoundValue::Literal(LiteralValue::Boolean(true)))
        );
    }

    #[test]
    fn negated_property_set_forward_reverse_and_combined() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let dict = Arc::new(InMemoryDictionary::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));

        let alice = dict.encode_node("http://ex.org/alice");
        let bob = dict.encode_node("http://ex.org/bob");
        let carol = dict.encode_node("http://ex.org/carol");
        let person = dict.encode_node("http://ex.org/Person");
        let knows = Iri::new("http://ex.org/knows");
        let likes = Iri::new("http://ex.org/likes");

        let txn = TxnId::new(40);
        repo.insert(
            txn,
            Triple {
                subject: alice,
                predicate: knows.clone(),
                object: Term::Iri(Iri::new("http://ex.org/bob")),
            },
        )
        .unwrap();
        repo.insert(
            txn,
            Triple {
                subject: alice,
                predicate: likes.clone(),
                object: Term::Iri(Iri::new("http://ex.org/carol")),
            },
        )
        .unwrap();
        repo.insert(
            txn,
            Triple {
                subject: bob,
                predicate: Iri::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
                object: Term::Iri(Iri::new("http://ex.org/Person")),
            },
        )
        .unwrap();
        engine.commit_transaction(txn).unwrap();

        let p = standard_pipeline_with_dictionary(repo, dict);

        // Forward negated set: every triple whose predicate is not `knows`.
        let r = p
            .execute(&QueryRequest::new(
                "SELECT ?s ?o WHERE { ?s !<http://ex.org/knows> ?o }",
            ))
            .unwrap();
        let pairs: Vec<(BoundValue, BoundValue)> = r
            .solutions
            .iter()
            .map(|s| (s.get("s").unwrap().clone(), s.get("o").unwrap().clone()))
            .collect();
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&(BoundValue::Node(alice), BoundValue::Node(carol))));
        assert!(pairs.contains(&(BoundValue::Node(bob), BoundValue::Node(person))));

        // Reverse negated set `!^knows`: (y, x) for triples (y, p, x) with p != knows.
        let r = p
            .execute(&QueryRequest::new(
                "SELECT ?s ?o WHERE { ?s !^<http://ex.org/knows> ?o }",
            ))
            .unwrap();
        let pairs: Vec<(BoundValue, BoundValue)> = r
            .solutions
            .iter()
            .map(|s| (s.get("s").unwrap().clone(), s.get("o").unwrap().clone()))
            .collect();
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&(BoundValue::Node(carol), BoundValue::Node(alice))));
        assert!(pairs.contains(&(BoundValue::Node(person), BoundValue::Node(bob))));

        // Combined `!(knows|^likes)`: forward non-knows plus reverse non-likes.
        let r = p
            .execute(&QueryRequest::new(
                "SELECT ?s ?o WHERE { ?s !(<http://ex.org/knows>|^<http://ex.org/likes>) ?o }",
            ))
            .unwrap();
        assert_eq!(r.solutions.len(), 4);

        // `!a` excludes only rdf:type in the forward direction.
        let r = p
            .execute(&QueryRequest::new("SELECT ?s ?o WHERE { ?s !a ?o }"))
            .unwrap();
        assert_eq!(r.solutions.len(), 2);
        assert!(
            r.solutions
                .iter()
                .any(|s| s.get("o") == Some(&BoundValue::Node(carol)))
        );

        // `!^a` excludes rdf:type in the reverse direction only.
        let r = p
            .execute(&QueryRequest::new("SELECT ?s ?o WHERE { ?s !^a ?o }"))
            .unwrap();
        let pairs: Vec<(BoundValue, BoundValue)> = r
            .solutions
            .iter()
            .map(|s| (s.get("s").unwrap().clone(), s.get("o").unwrap().clone()))
            .collect();
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&(BoundValue::Node(bob), BoundValue::Node(alice))));
        assert!(pairs.contains(&(BoundValue::Node(carol), BoundValue::Node(alice))));
    }

    #[test]
    fn path_zero_length_question_mark_constant_endpoints_on_empty_data() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let dict = Arc::new(InMemoryDictionary::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));

        let o_node = dict.encode_node("http://example/o");
        let s_node = dict.encode_node("http://example/s");
        let p = standard_pipeline_with_dictionary(repo, dict);

        let r = p
            .execute(&QueryRequest::new(
                "PREFIX : <http://example/> SELECT ?s WHERE { ?s :p? :o }",
            ))
            .unwrap();
        assert_eq!(r.solutions.len(), 1);
        assert_eq!(r.solutions[0].get("s"), Some(&BoundValue::Node(o_node)));

        let r = p
            .execute(&QueryRequest::new(
                "PREFIX : <http://example/> SELECT ?o WHERE { :s :p? ?o }",
            ))
            .unwrap();
        assert_eq!(r.solutions.len(), 1);
        assert_eq!(r.solutions[0].get("o"), Some(&BoundValue::Node(s_node)));
    }

    #[test]
    fn path_zero_or_more_includes_literal_object_self_pair() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let dict = Arc::new(InMemoryDictionary::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));

        let alice = dict.encode_node("http://ex.org/a");
        let bob = dict.encode_node("http://ex.org/bob");

        let txn = TxnId::new(41);
        repo.insert(
            txn,
            Triple {
                subject: alice,
                predicate: Iri::new("http://ex.org/knows"),
                object: Term::Iri(Iri::new("http://ex.org/bob")),
            },
        )
        .unwrap();
        repo.insert(
            txn,
            Triple {
                subject: alice,
                predicate: Iri::new("http://ex.org/name"),
                object: Term::Literal(LiteralValue::String("test".into())),
            },
        )
        .unwrap();
        engine.commit_transaction(txn).unwrap();

        let p = standard_pipeline_with_dictionary(repo, dict);
        let r = p
            .execute(&QueryRequest::new(
                "SELECT ?x ?y WHERE { ?x <http://ex.org/knows>* ?y }",
            ))
            .unwrap();
        assert_eq!(r.solutions.len(), 4);
        let pairs: Vec<(BoundValue, BoundValue)> = r
            .solutions
            .iter()
            .map(|s| (s.get("x").unwrap().clone(), s.get("y").unwrap().clone()))
            .collect();
        assert!(pairs.contains(&(BoundValue::Node(alice), BoundValue::Node(alice))));
        assert!(pairs.contains(&(BoundValue::Node(alice), BoundValue::Node(bob))));
        assert!(pairs.contains(&(
            BoundValue::Literal(LiteralValue::String("test".into())),
            BoundValue::Literal(LiteralValue::String("test".into()))
        )));
    }

    // ---- P5-03: enforced tenant scope (read isolation + write stamping) ----

    /// Pipeline seeded with one named-graph quad per tenant.
    fn tenant_seeded_pipeline(
    ) -> (
        Arc<InMemoryStorageEngine>,
        crate::application::QueryPipeline<
            SimpleQueryPlanner,
            CostBasedOptimizer<EngineQueryStatistics>,
            UpdateQueryExecutor<EngineUpdateWriteService>,
        >,
    ) {
        let (engine, dict, repo) = seed_update();
        let engine_clone = Arc::clone(&engine);
        let p = update_pipeline(engine, dict, repo);
        p.execute(&QueryRequest::new(
            "INSERT DATA { GRAPH <urn:tenant:acme> { <http://ex.org/carol> <http://ex.org/name> \"Carol\" } }",
        ))
        .unwrap();
        p.execute(&QueryRequest::new(
            "INSERT DATA { GRAPH <urn:tenant:other> { <http://ex.org/dave> <http://ex.org/name> \"Dave\" } }",
        ))
        .unwrap();
        (engine_clone, p)
    }

    #[test]
    fn tenant_scope_limits_default_graph_to_tenant() {
        let (_engine, p) = tenant_seeded_pipeline();

        // Unscoped: the shared default graph holds the seeded alice/bob triples.
        let r = p
            .execute(&QueryRequest::new(
                "SELECT ?s WHERE { ?s <http://ex.org/name> ?n }",
            ))
            .unwrap();
        assert_eq!(r.solutions.len(), 2, "unscoped sees the shared default graph");

        // Scoped: the default graph becomes the tenant graph only.
        let req = QueryRequest::new("SELECT ?s WHERE { ?s <http://ex.org/name> ?n }")
            .with_tenant_scope(TenantScope::new("acme"));
        let r = p.execute(&req).unwrap();
        assert_eq!(
            r.solutions.len(),
            1,
            "acme must see only its own tenant graph (carol), not alice/bob/dave"
        );

        let other = QueryRequest::new("SELECT ?s WHERE { ?s <http://ex.org/name> ?n }")
            .with_tenant_scope(TenantScope::new("other"));
        let r = p.execute(&other).unwrap();
        assert_eq!(r.solutions.len(), 1, "other sees only its own tenant graph (dave)");
    }

    #[test]
    fn tenant_scope_rejects_foreign_graph_references() {
        let (_engine, p) = tenant_seeded_pipeline();
        let scope = TenantScope::new("acme");

        let err = p
            .execute(
                &QueryRequest::new("SELECT * WHERE { GRAPH <urn:tenant:other> { ?s ?p ?o } }")
                    .with_tenant_scope(scope.clone()),
            )
            .unwrap_err();
        assert!(
            err.message().starts_with("forbidden"),
            "GRAPH to foreign tenant must be forbidden, got: {err}"
        );

        let err = p
            .execute(
                &QueryRequest::new("SELECT * FROM <urn:tenant:other> WHERE { ?s ?p ?o }")
                    .with_tenant_scope(scope),
            )
            .unwrap_err();
        assert!(
            err.message().starts_with("forbidden"),
            "FROM foreign tenant must be forbidden, got: {err}"
        );
    }

    #[test]
    fn tenant_scope_update_stamps_into_tenant_graph() {
        let (engine, p) = tenant_seeded_pipeline();

        // Default-graph INSERT DATA is re-pointed at the tenant graph.
        let r = p
            .execute(
                &QueryRequest::new(
                    "INSERT DATA { <http://ex.org/eve> <http://ex.org/name> \"Eve\" }",
                )
                .with_tenant_scope(TenantScope::new("acme")),
            )
            .unwrap();
        assert_eq!(r.affected, 1);

        let acme_graph = Iri::new("urn:tenant:acme");
        let acme_quads: Vec<_> = engine
            .named_graph_quads()
            .into_iter()
            .filter(|q| q.graph_name.as_ref() == Some(&acme_graph))
            .collect();
        assert_eq!(acme_quads.len(), 2, "acme graph holds carol + eve");

        // The shared default graph is untouched by scoped writes.
        let default = engine.default_graph_triples_in_txn(None);
        assert_eq!(default.len(), 2, "alice/bob remain in the shared default graph");

        // A scoped read sees the stamped data.
        let req = QueryRequest::new("SELECT ?n WHERE { <http://ex.org/eve> <http://ex.org/name> ?n }")
            .with_tenant_scope(TenantScope::new("acme"));
        let r = p.execute(&req).unwrap();
        assert_eq!(r.solutions.len(), 1);

        // Explicit foreign-graph writes are rejected.
        let err = p
            .execute(
                &QueryRequest::new(
                    "INSERT DATA { GRAPH <urn:tenant:other> { <http://ex.org/x> <http://ex.org/name> \"X\" } }",
                )
                .with_tenant_scope(TenantScope::new("acme")),
            )
            .unwrap_err();
        assert!(
            err.message().starts_with("forbidden"),
            "write to foreign tenant graph must be forbidden, got: {err}"
        );
    }
}
