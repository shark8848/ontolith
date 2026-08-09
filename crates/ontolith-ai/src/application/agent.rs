//! P8-03: agent tool adapters over the semantic search service.
//!
//! [`SemanticRetrievalTool`] is the reference implementation of the
//! `plugin-api` [`AgentTool`] contract: agents discover it via
//! [`AgentTool::definition`] (capability `Retrieval`) and invoke it with
//! string-typed arguments. The tool is deterministic and side-effect free —
//! same query, same byte-level output (R4 KPI).

use std::sync::Arc;

use ontolith_core::error::OntolithError;
use ontolith_plugin_api::domain::{
    AgentTool, PluginCapability, RetrievalHit, RetrievalResult, ToolDefinition, ToolInput,
    ToolOutput, ToolParam,
};
use ontolith_rdf::domain::Term;

use super::SemanticSearchService;

/// Reference retrieval tool: natural-language query -> top-k related terms.
pub struct SemanticRetrievalTool {
    service: Arc<SemanticSearchService>,
    definition: ToolDefinition,
}

impl SemanticRetrievalTool {
    pub fn new(service: Arc<SemanticSearchService>) -> Self {
        Self {
            service,
            definition: ToolDefinition {
                name: "semantic_retrieval".to_owned(),
                description: "Semantic term retrieval over the RDF store: returns top-k "
                    .to_owned()
                    + "terms related to a natural-language query. Approximate recall; "
                    + "verification still goes through SPARQL/SHACL.",
                parameters: vec![
                    ToolParam {
                        name: "q".to_owned(),
                        description: "query text".to_owned(),
                        required: true,
                    },
                    ToolParam {
                        name: "k".to_owned(),
                        description: "max hits [1,100]".to_owned(),
                        required: false,
                    },
                ],
                capabilities: vec![PluginCapability::Retrieval],
            },
        }
    }
}

impl AgentTool for SemanticRetrievalTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn call(&self, input: &ToolInput) -> Result<ToolOutput, OntolithError> {
        let q = input.get_required("q")?;
        if q.trim().is_empty() {
            return Err(OntolithError::failed(
                "tool parameter `q` must not be empty",
            ));
        }
        let k = match input.get("k") {
            Some(raw) => raw.parse::<usize>().map_err(|_| {
                OntolithError::failed(format!(
                    "tool parameter `k` must be an integer, got `{raw}`"
                ))
            })?,
            None => crate::domain::DEFAULT_TOP_K,
        };
        let hits = self.service.search_text(q, k)?;
        Ok(ToolOutput::Retrieval(RetrievalResult {
            query: q.to_owned(),
            hits: hits
                .into_iter()
                .map(|hit| RetrievalHit {
                    term: term_text(&hit.term),
                    kind: term_kind(&hit.term).to_owned(),
                    score: hit.score,
                })
                .collect(),
        }))
    }
}

fn term_kind(term: &Term) -> &'static str {
    match term {
        Term::Iri(_) => "uri",
        Term::Literal(_) => "literal",
        Term::BlankNode(_) => "bnode",
    }
}

/// Human-readable term text for agent-facing output (IRI / literal lexical
/// form / blank node id), matching the HTTP result rendering contract.
fn term_text(term: &Term) -> String {
    match term {
        Term::Iri(iri) => iri.as_str().to_owned(),
        Term::Literal(lit) => lit.lexical_form(),
        Term::BlankNode(id) => format!("_:n{}", id.get()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::EmbeddingProvider;
    use crate::infrastructure::FeatureHashEmbedding;
    use ontolith_plugin_api::domain::ToolInput;

    fn tool() -> SemanticRetrievalTool {
        let provider = Arc::new(FeatureHashEmbedding::default()) as Arc<dyn EmbeddingProvider>;
        let mut service = SemanticSearchService::new(provider);
        let terms: Vec<Term> = (0..100)
            .map(|i| Term::iri(format!("urn:ex:term_{i}_query")))
            .chain([
                Term::iri("urn:ex:sparql_query"),
                Term::iri("urn:ex:email_address"),
            ])
            .collect();
        service.index_terms(&terms).unwrap();
        SemanticRetrievalTool::new(Arc::new(service))
    }

    #[test]
    fn definition_advertises_retrieval_and_parameters() {
        let tool = tool();
        let def = tool.definition();
        assert_eq!(def.name, "semantic_retrieval");
        assert!(def.capabilities.contains(&PluginCapability::Retrieval));
        assert!(def.parameters.iter().any(|p| p.name == "q" && p.required));
        assert!(def.parameters.iter().any(|p| p.name == "k" && !p.required));
    }

    #[test]
    fn call_returns_top_related_hits_and_is_deterministic() {
        let tool = tool();
        let input = ToolInput {
            args: vec![
                ("q".to_owned(), "sparql".to_owned()),
                ("k".to_owned(), "3".to_owned()),
            ],
        };
        let first = tool.call(&input).unwrap();
        let second = tool.call(&input).unwrap();
        assert_eq!(format!("{first:?}"), format!("{second:?}"));
        match (&first, &second) {
            (ToolOutput::Retrieval(a), ToolOutput::Retrieval(b)) => {
                assert_eq!(a, b);
                assert!(!a.hits.is_empty());
                assert!(
                    a.hits[0].term.contains("sparql_query"),
                    "got {:?}",
                    a.hits[0]
                );
                assert_eq!(a.hits[0].kind, "uri");
                assert!(a.hits[0].score >= a.hits.last().unwrap().score);
            }
            _ => panic!("expected retrieval output, got {first:?}"),
        }
    }

    #[test]
    fn call_validates_required_and_invalid_arguments() {
        let tool = tool();
        let err = tool.call(&ToolInput::default()).unwrap_err();
        assert!(
            err.message()
                .contains("missing required tool parameter `q`")
        );
        let err = tool
            .call(&ToolInput {
                args: vec![("q".to_owned(), "   ".to_owned())],
            })
            .unwrap_err();
        assert!(err.message().contains("must not be empty"));
        let err = tool
            .call(&ToolInput {
                args: vec![
                    ("q".to_owned(), "sparql".to_owned()),
                    ("k".to_owned(), "many".to_owned()),
                ],
            })
            .unwrap_err();
        assert!(err.message().contains("must be an integer"));
    }
}
