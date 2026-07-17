use ontolith_query::domain::{QueryRequest, QueryResultSummary};
use ontolith_rdf::domain::Dataset;

#[derive(Debug, Clone, PartialEq)]
pub enum SdkOperation {
    Query(QueryRequest),
    Ingest(Dataset),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SdkRequest {
    pub request_id: String,
    pub operation: SdkOperation,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SdkResponse {
    QueryAccepted { summary: QueryResultSummary },
    IngestAccepted { triples: usize },
    Error { message: String },
}

pub fn status() -> &'static str {
    "domain"
}
