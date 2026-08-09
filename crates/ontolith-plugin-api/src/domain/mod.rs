use ontolith_core::error::OntolithError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PluginId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginCapability {
    StorageBackend,
    Parser,
    Optimizer,
    Reasoner,
    SecurityProvider,
    /// Semantic retrieval over the RDF store (P8-02/P8-03).
    Retrieval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: PluginId,
    pub version: String,
    pub api_version: String,
    pub capabilities: Vec<PluginCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginContext {
    pub tenant: Option<String>,
    pub trace_id: Option<String>,
}

pub trait Plugin {
    fn manifest(&self) -> &PluginManifest;
    fn initialize(&mut self, context: PluginContext) -> Result<(), OntolithError>;
}

pub fn status() -> &'static str {
    "domain"
}

/// P8-03: agent tool parameter declaration (self-describing input contract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolParam {
    pub name: String,
    pub description: String,
    pub required: bool,
}

/// Agent tool definition: stable identity + human/LLM-facing description.
///
/// `capabilities` advertises which plugin capabilities back the tool (e.g.
/// [`PluginCapability::Retrieval`]), so agents can match tools to needs
/// without loading the plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Vec<ToolParam>,
    pub capabilities: Vec<PluginCapability>,
}

/// Raw tool invocation arguments (`name -> value` pairs, P8-03 contract).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolInput {
    pub args: Vec<(String, String)>,
}

impl ToolInput {
    /// Look up one argument by name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.args
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Required-argument lookup with a stable, agent-facing error.
    pub fn get_required(&self, name: &str) -> Result<&str, OntolithError> {
        self.get(name).ok_or_else(|| {
            OntolithError::failed(format!("missing required tool parameter `{name}`"))
        })
    }
}

/// One retrieval hit rendered for an agent: canonical term text + RDF term
/// kind (`uri` | `literal` | `bnode`) + cosine score.
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalHit {
    pub term: String,
    pub kind: String,
    pub score: f32,
}

/// Structured semantic-retrieval tool output (P8-03).
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalResult {
    pub query: String,
    pub hits: Vec<RetrievalHit>,
}

/// Agent tool output: plain text or a structured retrieval payload.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutput {
    Text(String),
    Retrieval(RetrievalResult),
}

/// Agent tool contract (P8-03): a self-describing, deterministic callable
/// unit that agents can discover via [`AgentTool::definition`] and invoke
/// with string-typed arguments. Tools must be side-effect free for a given
/// input (determinism is part of the R4 KPI).
pub trait AgentTool: Send + Sync {
    fn definition(&self) -> &ToolDefinition;
    fn call(&self, input: &ToolInput) -> Result<ToolOutput, OntolithError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retrieval_definition() -> ToolDefinition {
        ToolDefinition {
            name: "semantic_retrieval".to_owned(),
            description: "top-k related terms for a query".to_owned(),
            parameters: vec![
                ToolParam {
                    name: "q".to_owned(),
                    description: "query text".to_owned(),
                    required: true,
                },
                ToolParam {
                    name: "k".to_owned(),
                    description: "max hits".to_owned(),
                    required: false,
                },
            ],
            capabilities: vec![PluginCapability::Retrieval],
        }
    }

    #[test]
    fn tool_definition_advertises_retrieval_capability() {
        let def = retrieval_definition();
        assert!(def.capabilities.contains(&PluginCapability::Retrieval));
        assert_eq!(def.parameters.iter().filter(|p| p.required).count(), 1);
    }

    #[test]
    fn tool_input_lookup_and_required_validation() {
        let input = ToolInput {
            args: vec![("q".to_owned(), "sparql".to_owned())],
        };
        assert_eq!(input.get("q"), Some("sparql"));
        assert_eq!(input.get("k"), None);
        assert_eq!(input.get_required("q").unwrap(), "sparql");
        let err = input.get_required("k").unwrap_err();
        assert!(
            err.message()
                .contains("missing required tool parameter `k`")
        );
    }

    #[test]
    fn retrieval_hit_roundtrip_is_deterministic() {
        let result = RetrievalResult {
            query: "sparql".to_owned(),
            hits: vec![
                RetrievalHit {
                    term: "urn:ex:sparql_query".to_owned(),
                    kind: "uri".to_owned(),
                    score: 0.9,
                },
                RetrievalHit {
                    term: "urn:ex:rdf_graph".to_owned(),
                    kind: "uri".to_owned(),
                    score: 0.8,
                },
            ],
        };
        let a = format!("{result:?}");
        let b = format!("{result:?}");
        assert_eq!(a, b, "tool output must be byte-deterministic");
    }

    #[test]
    fn empty_input_is_a_valid_default() {
        let input = ToolInput::default();
        assert!(input.args.is_empty());
        assert!(input.get("q").is_none());
    }
}
