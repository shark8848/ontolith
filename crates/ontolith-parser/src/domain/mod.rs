//! Parser domain types (L3 — full RDF syntax surface).

use ontolith_rdf::domain::{Dataset, Quad, Triple};

/// Concrete RDF syntax supported by the parser surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseFormat {
    Turtle,
    TriG,
    NTriples,
    NQuads,
    JsonLd,
}

impl ParseFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Turtle => "turtle",
            Self::TriG => "trig",
            Self::NTriples => "n-triples",
            Self::NQuads => "n-quads",
            Self::JsonLd => "json-ld",
        }
    }

    pub const fn is_implemented(self) -> bool {
        !matches!(self, Self::JsonLd)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseRequest {
    pub format: ParseFormat,
    pub source_name: String,
    /// Optional base IRI for relative resolution (`@base` / `BASE` may override).
    pub base_iri: Option<String>,
}

impl ParseRequest {
    pub fn new(format: ParseFormat, source_name: impl Into<String>) -> Self {
        Self {
            format,
            source_name: source_name.into(),
            base_iri: None,
        }
    }

    pub fn ntriples(source_name: impl Into<String>) -> Self {
        Self::new(ParseFormat::NTriples, source_name)
    }

    pub fn nquads(source_name: impl Into<String>) -> Self {
        Self::new(ParseFormat::NQuads, source_name)
    }

    pub fn turtle(source_name: impl Into<String>) -> Self {
        Self::new(ParseFormat::Turtle, source_name)
    }

    pub fn trig(source_name: impl Into<String>) -> Self {
        Self::new(ParseFormat::TriG, source_name)
    }

    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base_iri = Some(base.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParseStats {
    pub triple_count: usize,
    pub quad_count: usize,
    pub line_count: usize,
    pub skipped_comments: usize,
    pub prefix_count: usize,
    pub blank_nodes_minted: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseOutput {
    pub dataset: Dataset,
    pub stats: ParseStats,
}

/// Streaming parse events for ingest pipelines.
#[derive(Debug, Clone, PartialEq)]
pub enum RdfEvent {
    Triple(Triple),
    Quad(Quad),
    Prefix { prefix: String, iri: String },
    Base(String),
    Comment,
}

/// Sink for streaming parse (no intermediate full document required by caller).
pub trait RdfEventSink {
    fn on_event(&mut self, event: RdfEvent) -> Result<(), ontolith_core::error::OntolithError>;
}

/// Collects events into a [`Dataset`].
#[derive(Debug, Default)]
pub struct DatasetSink {
    pub dataset: Dataset,
    pub stats: ParseStats,
}

impl RdfEventSink for DatasetSink {
    fn on_event(&mut self, event: RdfEvent) -> Result<(), ontolith_core::error::OntolithError> {
        match event {
            RdfEvent::Triple(t) => {
                self.dataset.insert_default(t);
                self.stats.triple_count += 1;
            }
            RdfEvent::Quad(q) => {
                let is_named = q.graph_name.is_some();
                self.dataset.insert_quad(q);
                if is_named {
                    self.stats.quad_count += 1;
                } else {
                    self.stats.triple_count += 1;
                    self.stats.quad_count += 1;
                }
            }
            RdfEvent::Prefix { .. } => self.stats.prefix_count += 1,
            RdfEvent::Base(_) => {}
            RdfEvent::Comment => self.stats.skipped_comments += 1,
        }
        Ok(())
    }
}

pub fn status() -> &'static str {
    "domain"
}
