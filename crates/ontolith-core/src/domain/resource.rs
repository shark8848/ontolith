//! Resource model: IRI, Blank Node, Literal (SAS-0401 §5).
//!
//! Existing storage/query code continues to use [`NodeId`], [`Iri`],
//! [`BlankNodeId`], and [`LiteralValue`] directly. The richer types in this
//! module are the normative Knowledge Object surface.

use crate::domain::NodeId;
use crate::domain::canonical::{CanonicalEncode, CanonicalWriter};
use crate::error::OntolithError;

/// Internationalized Resource Identifier (absolute form expected at API edges).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Iri(pub String);

impl Iri {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Validate a non-empty IRI-like string.
    ///
    /// Full RFC 3987 validation is deferred; this enforces the R1 baseline:
    /// non-empty, no ASCII whitespace, and must contain `:` (scheme separator).
    pub fn parse(value: impl Into<String>) -> Result<Self, OntolithError> {
        let value = value.into();
        if value.is_empty() {
            return Err(OntolithError::InvalidArgument("iri must not be empty"));
        }
        if value.chars().any(|c| c.is_ascii_whitespace()) {
            return Err(OntolithError::InvalidArgument(
                "iri must not contain whitespace",
            ));
        }
        if !value.contains(':') {
            return Err(OntolithError::InvalidArgument(
                "iri must include a scheme separator ':'",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Iri {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for Iri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl CanonicalEncode for Iri {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_tag(b"I");
        out.write_str(self.as_str());
    }
}

/// Blank node label. Scope is dataset-local unless lifted by import policy.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlankNodeId(pub String);

impl BlankNodeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, OntolithError> {
        let value = value.into();
        if value.is_empty() {
            return Err(OntolithError::InvalidArgument(
                "blank node id must not be empty",
            ));
        }
        if value.chars().any(|c| c.is_ascii_whitespace()) {
            return Err(OntolithError::InvalidArgument(
                "blank node id must not contain whitespace",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for BlankNodeId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for BlankNodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "_:{}", self.as_str())
    }
}

impl CanonicalEncode for BlankNodeId {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_tag(b"B");
        out.write_str(self.as_str())
    }
}

/// BCP 47 language tag (lowercase canonical form).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LanguageTag(String);

impl LanguageTag {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, OntolithError> {
        let raw = value.as_ref().trim();
        if raw.is_empty() {
            return Err(OntolithError::InvalidArgument(
                "language tag must not be empty",
            ));
        }
        if !raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(OntolithError::InvalidArgument(
                "language tag contains invalid characters",
            ));
        }
        Ok(Self(raw.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LanguageTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compact literal payload used by current storage/query paths.
///
/// Prefer [`Literal`] when datatype / language must be preserved.
#[derive(Debug, Clone, PartialEq)]
pub enum LiteralValue {
    String(String),
    Integer(i64),
    Decimal(f64),
    Boolean(bool),
}

impl LiteralValue {
    pub fn lexical_form(&self) -> String {
        match self {
            Self::String(v) => v.clone(),
            Self::Integer(v) => v.to_string(),
            Self::Decimal(v) => format_decimal_bits(*v),
            Self::Boolean(v) => {
                if *v {
                    "true".to_owned()
                } else {
                    "false".to_owned()
                }
            }
        }
    }

    pub fn xsd_datatype_iri(&self) -> Iri {
        match self {
            Self::String(_) => Iri::new("http://www.w3.org/2001/XMLSchema#string"),
            Self::Integer(_) => Iri::new("http://www.w3.org/2001/XMLSchema#integer"),
            Self::Decimal(_) => Iri::new("http://www.w3.org/2001/XMLSchema#double"),
            Self::Boolean(_) => Iri::new("http://www.w3.org/2001/XMLSchema#boolean"),
        }
    }
}

fn format_decimal_bits(value: f64) -> String {
    // Deterministic lexical form: prefer simple Display; fall back to bit pattern.
    let simple = format!("{value}");
    if simple.contains('e') || simple.contains('E') || simple == "NaN" || simple.contains("inf") {
        format!("bits:{}", value.to_bits())
    } else {
        simple
    }
}

impl CanonicalEncode for LiteralValue {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        match self {
            Self::String(v) => {
                out.write_tag(b"LS");
                out.write_str(v);
            }
            Self::Integer(v) => {
                out.write_tag(b"LI");
                out.write_str(&v.to_string());
            }
            Self::Decimal(v) => {
                out.write_tag(b"LD");
                out.write_u64(v.to_bits());
            }
            Self::Boolean(v) => {
                out.write_tag(b"LB");
                out.write_str(if *v { "true" } else { "false" });
            }
        }
    }
}

/// Full RDF literal with optional language tag or explicit datatype.
#[derive(Debug, Clone, PartialEq)]
pub struct Literal {
    pub value: LiteralValue,
    pub datatype: Iri,
    pub language: Option<LanguageTag>,
}

impl Literal {
    pub fn new(value: LiteralValue) -> Self {
        let datatype = value.xsd_datatype_iri();
        Self {
            value,
            datatype,
            language: None,
        }
    }

    pub fn string(value: impl Into<String>) -> Self {
        Self::new(LiteralValue::String(value.into()))
    }

    pub fn language_string(value: impl Into<String>, language: LanguageTag) -> Self {
        Self {
            value: LiteralValue::String(value.into()),
            datatype: Iri::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#langString"),
            language: Some(language),
        }
    }

    pub fn typed(value: LiteralValue, datatype: Iri) -> Self {
        Self {
            value,
            datatype,
            language: None,
        }
    }

    pub fn lexical_form(&self) -> String {
        self.value.lexical_form()
    }
}

impl CanonicalEncode for Literal {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_tag(b"L");
        self.value.write_canonical(out);
        out.write_tag(b"^");
        out.write_str(self.datatype.as_str());
        if let Some(lang) = &self.language {
            out.write_tag(b"@");
            out.write_str(lang.as_str());
        }
    }
}

/// RDF Resource: IRI | Blank Node | Literal (SAS-0401 §5).
#[derive(Debug, Clone, PartialEq)]
pub enum Resource {
    Iri(Iri),
    BlankNode(BlankNodeId),
    Literal(Literal),
}

impl Resource {
    pub fn iri(value: impl Into<String>) -> Result<Self, OntolithError> {
        Ok(Self::Iri(Iri::parse(value)?))
    }

    pub fn blank(value: impl Into<String>) -> Result<Self, OntolithError> {
        Ok(Self::BlankNode(BlankNodeId::parse(value)?))
    }

    pub fn literal(value: Literal) -> Self {
        Self::Literal(value)
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Iri(_) => "iri",
            Self::BlankNode(_) => "blank_node",
            Self::Literal(_) => "literal",
        }
    }
}

impl CanonicalEncode for Resource {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        match self {
            Self::Iri(v) => v.write_canonical(out),
            Self::BlankNode(v) => v.write_canonical(out),
            Self::Literal(v) => v.write_canonical(out),
        }
    }
}

/// Dictionary-bound resource handle: logical resource + stable node id.
///
/// `node_id` is immutable for the lifetime of the database epoch (SAS-0401 §5).
#[derive(Debug, Clone, PartialEq)]
pub struct BoundResource {
    pub node_id: NodeId,
    pub resource: Resource,
}

impl BoundResource {
    pub fn new(node_id: NodeId, resource: Resource) -> Self {
        Self { node_id, resource }
    }
}
