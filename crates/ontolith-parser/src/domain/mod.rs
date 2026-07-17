use ontolith_rdf::domain::Dataset;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseFormat {
    Turtle,
    TriG,
    NTriples,
    NQuads,
    JsonLd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseRequest {
    pub format: ParseFormat,
    pub source_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseStats {
    pub triple_count: usize,
    pub quad_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseOutput {
    pub dataset: Dataset,
    pub stats: ParseStats,
}

pub fn status() -> &'static str {
    "domain"
}
