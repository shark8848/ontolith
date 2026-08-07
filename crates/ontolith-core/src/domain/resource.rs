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

/// Literal payload used by storage/query paths.
///
/// The compact numeric/string variants carry their implicit XSD datatype
/// ([`LiteralValue::xsd_datatype_iri`]); [`LiteralValue::Lang`] and
/// [`LiteralValue::Typed`] preserve language tags and explicit datatypes
/// that the compact forms cannot represent.
#[derive(Debug, Clone, PartialEq)]
pub enum LiteralValue {
    /// Plain string literal (`xsd:string`); also the RDF 1.1 default for
    /// bare quoted strings.
    String(String),
    /// Language-tagged string (`rdf:langString`).
    Lang {
        value: String,
        lang: LanguageTag,
    },
    /// Typed literal with an explicit (possibly non-numeric) datatype,
    /// preserved lexically.
    Typed {
        value: String,
        datatype: Iri,
    },
    Integer(i64),
    /// `xsd:decimal`.
    Decimal(f64),
    /// `xsd:float`.
    Float(f32),
    /// `xsd:double`.
    Double(f64),
    Boolean(bool),
}

impl LiteralValue {
    pub fn lexical_form(&self) -> String {
        match self {
            Self::String(v) => v.clone(),
            Self::Lang { value, .. } => value.clone(),
            Self::Typed { value, .. } => value.clone(),
            Self::Integer(v) => v.to_string(),
            Self::Decimal(v) => format_decimal_bits(*v),
            Self::Float(v) => format_float_bits(*v),
            Self::Double(v) => format_double_bits(*v),
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
            Self::Lang { .. } => Iri::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#langString"),
            Self::Typed { datatype, .. } => datatype.clone(),
            Self::Integer(_) => Iri::new("http://www.w3.org/2001/XMLSchema#integer"),
            Self::Decimal(_) => Iri::new("http://www.w3.org/2001/XMLSchema#decimal"),
            Self::Float(_) => Iri::new("http://www.w3.org/2001/XMLSchema#float"),
            Self::Double(_) => Iri::new("http://www.w3.org/2001/XMLSchema#double"),
            Self::Boolean(_) => Iri::new("http://www.w3.org/2001/XMLSchema#boolean"),
        }
    }

    /// Language tag of a language-tagged literal, if any.
    pub fn language_tag(&self) -> Option<&LanguageTag> {
        match self {
            Self::Lang { lang, .. } => Some(lang),
            _ => None,
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

fn format_float_bits(value: f32) -> String {
    // XSD float canonical-ish form: shortest round-trip decimal, with the
    // exponent marker normalized to the `E` form used by the W3C suite.
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "INF".to_owned()
        } else {
            "-INF".to_owned()
        };
    }
    let simple = format!("{value}");
    if simple.contains('e') || simple.contains('E') {
        normalize_exp(&simple)
    } else {
        simple
    }
}

fn format_double_bits(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "INF".to_owned()
        } else {
            "-INF".to_owned()
        };
    }
    let simple = format!("{value}");
    if simple.contains('e') || simple.contains('E') {
        normalize_exp(&simple)
    } else {
        simple
    }
}

/// Convert a Rust exponent form like `1.02e4` into `1.02E4`.
fn normalize_exp(s: &str) -> String {
    let mut parts = s.splitn(2, ['e', 'E']);
    let mantissa = parts.next().unwrap_or("");
    let exponent = parts.next().unwrap_or("");
    format!("{mantissa}E{exponent}")
}

impl CanonicalEncode for LiteralValue {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        match self {
            Self::String(v) => {
                out.write_tag(b"LS");
                out.write_str(v);
            }
            Self::Lang { value, lang } => {
                out.write_tag(b"LG");
                out.write_str(value);
                out.write_str(lang.as_str());
            }
            Self::Typed { value, datatype } => {
                out.write_tag(b"LT");
                out.write_str(value);
                out.write_str(datatype.as_str());
            }
            Self::Integer(v) => {
                out.write_tag(b"LI");
                out.write_str(&v.to_string());
            }
            Self::Decimal(v) => {
                out.write_tag(b"LD");
                out.write_u64(v.to_bits());
            }
            Self::Float(v) => {
                out.write_tag(b"LF");
                out.write_u64(v.to_bits() as u64);
            }
            Self::Double(v) => {
                out.write_tag(b"LQ");
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
