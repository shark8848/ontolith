//! Parser application contracts (L3).

use crate::domain::{ParseOutput, ParseRequest, RdfEventSink};
use ontolith_core::error::OntolithError;
use ontolith_storage::application::DictionaryCodec;

/// Parse RDF text into a dataset or stream of events.
pub trait RdfParser: Send + Sync {
    fn parse(
        &self,
        request: &ParseRequest,
        input: &str,
        dictionary: &dyn DictionaryCodec,
    ) -> Result<ParseOutput, OntolithError>;

    /// Streaming parse: emit triples/quads without requiring the caller to
    /// buffer the entire dataset (the implementation may still scan once).
    fn parse_streaming(
        &self,
        request: &ParseRequest,
        input: &str,
        dictionary: &dyn DictionaryCodec,
        sink: &mut dyn RdfEventSink,
    ) -> Result<(), OntolithError>;
}

pub fn status() -> &'static str {
    "application"
}
