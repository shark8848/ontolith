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
