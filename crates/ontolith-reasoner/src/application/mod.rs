//! Reasoner application contracts (L6).

use crate::domain::{MaterializeOutcome, ReasoningTask, Rule, ValidationReport};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::Triple;
use ontolith_storage::application::DictionaryCodec;

/// Forward-chaining materialization surface for RDFS/OWL RL rules.
pub trait Reasoner: Send + Sync {
    /// Derive the closure of `input` under the supported rule set.
    fn materialize(
        &self,
        dict: &dyn DictionaryCodec,
        task: &ReasoningTask,
        input: &[Triple],
    ) -> Result<MaterializeOutcome, OntolithError>;

    fn supported_rules(&self) -> Vec<Rule>;

    fn is_enabled(&self) -> bool;
}

/// SHACL validation surface: shapes graph + data graph → validation report.
pub trait ShaclValidator: Send + Sync {
    /// Validate `data` against the shapes in `shapes` (both as RDF triples).
    fn validate(
        &self,
        dict: &dyn DictionaryCodec,
        shapes: &[Triple],
        data: &[Triple],
    ) -> Result<ValidationReport, OntolithError>;
}

pub fn status() -> &'static str {
    "application"
}
