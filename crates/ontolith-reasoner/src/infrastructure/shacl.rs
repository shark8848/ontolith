//! SHACL baseline validator (L6 — constraint subset).
//!
//! Supports node shapes and property shapes with the documented subset:
//! targets (`sh:targetClass` / `sh:targetNode` / `sh:targetSubjectsOf` /
//! `sh:targetObjectsOf` + implicit class target), constraints (`sh:class`,
//! `sh:datatype`, `sh:nodeKind`, `sh:minCount`/`sh:maxCount`,
//! `sh:minLength`/`sh:maxLength`, `sh:pattern`, `sh:in`, `sh:hasValue`,
//! `sh:languageIn`, `sh:uniqueLang`, `sh:node`, `sh:and`/`sh:or`/`sh:xone`/`sh:not`,
//! `sh:qualifiedValueShape` with
//! `sh:qualifiedMinCount`/`sh:qualifiedMaxCount`/`sh:qualifiedValueShapesDisjoint`,
//! numeric ranges (`sh:minInclusive`/`sh:maxInclusive`/`sh:minExclusive`/
//! `sh:maxExclusive`), property pairs (`sh:equals`/`sh:disjoint`/`sh:lessThan`/
//! `sh:lessThanOrEquals`), `sh:pattern` with `sh:flags`, `sh:closed` with
//! `sh:ignoredProperties`), severities and messages.
//! `sh:pattern` uses a small self-contained regex subset (literals, `.`,
//! `[...]`, `\d\w\s\D\W\S`, `*+?`, full-string match; no groups/alternation);
//! `sh:flags` supports `i` (case-insensitive).

use crate::application::ShaclValidator;
use crate::domain::{
    ConstraintComponent, NodeKind, PropertyShape, SHACL_NS, Severity, Shape, Target,
    ValidationReport, ValidationResult, shacl,
};
use ontolith_core::domain::{Iri, LanguageTag, LiteralValue, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_storage::application::DictionaryCodec;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
const RDFS_CLASS: &str = "http://www.w3.org/2000/01/rdf-schema#Class";
const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";
const XSD_DATE: &str = "http://www.w3.org/2001/XMLSchema#date";
const XSD_BOOLEAN: &str = "http://www.w3.org/2001/XMLSchema#boolean";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
const XSD_DECIMAL: &str = "http://www.w3.org/2001/XMLSchema#decimal";
const XSD_FLOAT: &str = "http://www.w3.org/2001/XMLSchema#float";
const XSD_DOUBLE: &str = "http://www.w3.org/2001/XMLSchema#double";
/// SHACL baseline validator.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShaclEngine;

impl ShaclEngine {
    pub fn new() -> Self {
        Self
    }
}

fn subject_key(dict: &dyn DictionaryCodec, node: NodeId) -> String {
    dict.decode_node(node)
        .unwrap_or_else(|| format!("_:n{}", node.get()))
}

fn term_key(dict: &dyn DictionaryCodec, term: &Term) -> String {
    match term {
        Term::Iri(iri) => iri.as_str().to_owned(),
        Term::BlankNode(id) => subject_key(dict, *id),
        Term::Literal(v) => match v.language_tag() {
            // Language tags participate in RDF term equality (P6-02).
            Some(tag) => format!(
                "literal:{}|{}|{}",
                v.lexical_form(),
                v.xsd_datatype_iri(),
                tag.as_str()
            ),
            None => format!("literal:{}|{}", v.lexical_form(), v.xsd_datatype_iri()),
        },
    }
}

fn term_iri(term: &Term) -> Option<String> {
    match term {
        Term::Iri(iri) => Some(iri.as_str().to_owned()),
        _ => None,
    }
}

fn literal_usize(term: &Term) -> Option<usize> {
    match term {
        Term::Literal(LiteralValue::Integer(n)) if *n >= 0 => Some(*n as usize),
        _ => None,
    }
}

fn literal_string(term: &Term) -> Option<String> {
    // SHACL string-based constraints operate on the literal's string value
    // (its lexical form), including language-tagged strings (P6-02).
    match term {
        Term::Literal(lv) => Some(lv.lexical_form()),
        _ => None,
    }
}

fn literal_bool(term: &Term) -> Option<bool> {
    match term {
        Term::Literal(LiteralValue::Boolean(b)) => Some(*b),
        _ => None,
    }
}

/// Length of the SPARQL `str()` representation of a term: full IRI for IRIs,
/// lexical form for literals. Blank nodes have no string representation.
fn term_str_len(v: &Term) -> Option<usize> {
    match v {
        Term::Iri(iri) => Some(iri.as_str().chars().count()),
        Term::Literal(lv) => Some(lv.lexical_form().chars().count()),
        Term::BlankNode(_) => None,
    }
}

/// Comparable value extracted from a literal: numbers compare numerically,
/// strings compare by code point order. Other terms are not comparable.
fn compare_terms(a: &Term, b: &Term) -> Option<Ordering> {
    match (a, b) {
        (Term::Literal(LiteralValue::Integer(x)), Term::Literal(LiteralValue::Integer(y))) => {
            Some(x.cmp(y))
        }
        (Term::Literal(LiteralValue::String(x)), Term::Literal(LiteralValue::String(y))) => {
            Some(x.cmp(y))
        }
        (Term::Literal(x), Term::Literal(y)) => {
            if x.xsd_datatype_iri().as_str() == XSD_DATE_TIME
                && y.xsd_datatype_iri().as_str() == XSD_DATE_TIME
            {
                return compare_date_times(&x.lexical_form(), &y.lexical_form());
            }
            numeric_value(x).zip(numeric_value(y)).and_then(|(u, v)| u.partial_cmp(&v))
        }
        _ => None,
    }
}

/// Parsed `xsd:dateTime` value with an optional timezone offset (minutes).
struct DateTimeValue {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    tz: Option<i32>,
}

impl DateTimeValue {
    /// Local fields as a sortable tuple (used when both sides lack a timezone).
    fn local(&self) -> (i32, u32, u32, u32, u32, u32) {
        (self.year, self.month, self.day, self.hour, self.minute, self.second)
    }

    /// Instant as seconds since the Unix epoch (used when both sides carry a
    /// timezone).
    fn instant(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * 86_400
            + i64::from(self.hour) * 3_600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
            - i64::from(self.tz.unwrap_or(0)) * 60
    }
}

/// Days since 1970-01-01 (Howard Hinnant's civil-from-days inverse).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let yy = if m <= 2 { y - 1 } else { y } as i64;
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn parse_timezone(s: &str) -> Option<i32> {
    if s == "Z" {
        return Some(0);
    }
    let (sign, body) = match s.strip_prefix('-') {
        Some(body) => (-1, body),
        None => (1, s.strip_prefix('+')?),
    };
    let (h, m) = body.split_once(':')?;
    Some(sign * (h.parse::<i32>().ok()? * 60 + m.parse::<i32>().ok()?))
}

fn parse_date_time(lex: &str) -> Option<DateTimeValue> {
    let (date, rest) = lex.split_once('T')?;
    let mut dp = date.split('-');
    let year: i32 = dp.next()?.parse().ok()?;
    let month: u32 = dp.next()?.parse().ok()?;
    let day: u32 = dp.next()?.parse().ok()?;
    if dp.next().is_some() {
        return None;
    }
    // Timezone lives at the tail: `Z` or `[+-]HH:MM`.
    let mut tz = None;
    let mut time = rest;
    for marker in ['Z', '+', '-'] {
        if let Some(idx) = rest.rfind(marker)
            && idx > 0
            && parse_timezone(&rest[idx..]).is_some()
        {
            tz = parse_timezone(&rest[idx..]);
            time = &rest[..idx];
            break;
        }
    }
    let mut tp = time.split(':');
    let hour: u32 = tp.next()?.parse().ok()?;
    let minute: u32 = tp.next()?.parse().ok()?;
    let sec_part = tp.next()?;
    if tp.next().is_some() {
        return None;
    }
    let second: u32 = sec_part.split('.').next()?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    Some(DateTimeValue { year, month, day, hour, minute, second, tz })
}

/// XSD dateTime ordering per SHACL/SPARQL: two values with explicit timezones
/// compare as instants; two without compare by local fields; a mix of the two
/// is not comparable (the value does not satisfy an order constraint).
fn compare_date_times(a_lex: &str, b_lex: &str) -> Option<Ordering> {
    let a = parse_date_time(a_lex)?;
    let b = parse_date_time(b_lex)?;
    match (a.tz, b.tz) {
        (Some(_), Some(_)) => Some(a.instant().cmp(&b.instant())),
        (None, None) => Some(a.local().cmp(&b.local())),
        _ => None,
    }
}

/// True when the lexical form of a typed literal is a valid value for its
/// declared datatype (SHACL `sh:datatype` requires well-formed literals).
fn lexical_valid_for_datatype(dt: &str, value: &str) -> bool {
    match dt {
        XSD_BOOLEAN => matches!(value, "true" | "false" | "1" | "0"),
        x if x == XSD_INTEGER || is_integer_family(x) => {
            let trimmed = value
                .strip_prefix(|c| c == '+' || c == '-')
                .unwrap_or(value);
            let numeric = !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit());
            numeric && integer_in_range(dt, value)
        }
        XSD_DECIMAL => {
            let body = value
                .strip_prefix(|c| c == '+' || c == '-')
                .unwrap_or(value);
            !body.is_empty()
                && body.chars().all(|c| c.is_ascii_digit() || c == '.')
                && body.chars().filter(|c| *c == '.').count() <= 1
                && body.chars().any(|c| c.is_ascii_digit())
        }
        XSD_FLOAT | XSD_DOUBLE => {
            matches!(value, "INF" | "-INF" | "NaN")
                || value.parse::<f64>().is_ok()
                || value.parse::<f32>().is_ok()
        }
        XSD_DATE_TIME => parse_date_time(value).is_some(),
        XSD_DATE => {
            let mut parts = value.split('-');
            let Ok(y) = parts.next().unwrap_or("").parse::<i32>() else {
                return false;
            };
            let Ok(m) = parts.next().unwrap_or("").parse::<u32>() else {
                return false;
            };
            let Ok(d) = parts.next().unwrap_or("").parse::<u32>() else {
                return false;
            };
            parts.next().is_none() && (1..=12).contains(&m) && (1..=31).contains(&d) && y != 0
        }
        _ => true,
    }
}

fn is_integer_family(dt: &str) -> bool {
    matches!(
        dt,
        "http://www.w3.org/2001/XMLSchema#int"
            | "http://www.w3.org/2001/XMLSchema#long"
            | "http://www.w3.org/2001/XMLSchema#short"
            | "http://www.w3.org/2001/XMLSchema#byte"
            | "http://www.w3.org/2001/XMLSchema#nonNegativeInteger"
            | "http://www.w3.org/2001/XMLSchema#nonPositiveInteger"
            | "http://www.w3.org/2001/XMLSchema#negativeInteger"
            | "http://www.w3.org/2001/XMLSchema#positiveInteger"
            | "http://www.w3.org/2001/XMLSchema#unsignedLong"
            | "http://www.w3.org/2001/XMLSchema#unsignedInt"
            | "http://www.w3.org/2001/XMLSchema#unsignedShort"
            | "http://www.w3.org/2001/XMLSchema#unsignedByte"
    )
}

fn integer_in_range(dt: &str, value: &str) -> bool {
    let Ok(n) = value.parse::<i64>() else {
        return false;
    };
    match dt {
        "http://www.w3.org/2001/XMLSchema#byte" => (-128..=127).contains(&n),
        "http://www.w3.org/2001/XMLSchema#short" => (-32_768..=32_767).contains(&n),
        "http://www.w3.org/2001/XMLSchema#int" => (i32::MIN as i64..=i32::MAX as i64).contains(&n),
        "http://www.w3.org/2001/XMLSchema#unsignedByte" => (0..=255).contains(&n),
        "http://www.w3.org/2001/XMLSchema#unsignedShort" => (0..=65_535).contains(&n),
        "http://www.w3.org/2001/XMLSchema#unsignedInt" => (0..=u32::MAX as i64).contains(&n),
        "http://www.w3.org/2001/XMLSchema#unsignedLong" => n >= 0,
        "http://www.w3.org/2001/XMLSchema#nonNegativeInteger" => n >= 0,
        "http://www.w3.org/2001/XMLSchema#positiveInteger" => n > 0,
        "http://www.w3.org/2001/XMLSchema#nonPositiveInteger" => n <= 0,
        "http://www.w3.org/2001/XMLSchema#negativeInteger" => n < 0,
        _ => true,
    }
}


/// Numeric value of a literal for cross-datatype ordering (integer/decimal/
/// float/double compare by their numeric value, per XSD value-space ordering).
fn numeric_value(lv: &LiteralValue) -> Option<f64> {
    match lv {
        LiteralValue::Integer(i) => Some(*i as f64),
        LiteralValue::Decimal(f) | LiteralValue::Double(f) => Some(*f),
        LiteralValue::Float(f) => Some(*f as f64),
        _ => None,
    }
}

fn value_ge(v: &Term, limit: &Term) -> bool {
    matches!(
        compare_terms(v, limit),
        Some(Ordering::Greater | Ordering::Equal)
    )
}

fn value_le(v: &Term, limit: &Term) -> bool {
    matches!(
        compare_terms(v, limit),
        Some(Ordering::Less | Ordering::Equal)
    )
}

fn value_gt(v: &Term, limit: &Term) -> bool {
    compare_terms(v, limit) == Some(Ordering::Greater)
}

fn value_lt(v: &Term, limit: &Term) -> bool {
    compare_terms(v, limit) == Some(Ordering::Less)
}

/// Index shapes-graph triples by subject key.
fn index_triples(
    dict: &dyn DictionaryCodec,
    triples: &[Triple],
) -> HashMap<String, Vec<(String, Term)>> {
    let mut map: HashMap<String, Vec<(String, Term)>> = HashMap::new();
    for t in triples {
        map.entry(subject_key(dict, t.subject))
            .or_default()
            .push((t.predicate.as_str().to_owned(), t.object.clone()));
    }
    map
}

fn is_shacl_body(triples: &[(String, Term)]) -> bool {
    triples.iter().any(|(p, _)| p.starts_with(SHACL_NS))
}

fn parse_severity(term: &Term) -> Option<Severity> {
    // SHACL allows any IRI as a severity; the three well-known ones map to
    // the enum variants, everything else is preserved as a custom severity.
    match term {
        Term::Iri(iri) => {
            let key = iri.as_str();
            if key == shacl("Violation") {
                Some(Severity::Violation)
            } else if key == shacl("Warning") {
                Some(Severity::Warning)
            } else if key == shacl("Info") {
                Some(Severity::Info)
            } else {
                Some(Severity::Custom(key.to_owned()))
            }
        }
        _ => None,
    }
}

fn parse_node_kind(dict: &dyn DictionaryCodec, term: &Term) -> Option<NodeKind> {
    let key = term_key(dict, term);
    match key.as_str() {
        x if x == shacl("IRI") => Some(NodeKind::Iri),
        x if x == shacl("BlankNode") => Some(NodeKind::BlankNode),
        x if x == shacl("Literal") => Some(NodeKind::Literal),
        x if x == shacl("BlankNodeOrIRI") => Some(NodeKind::BlankNodeOrIri),
        x if x == shacl("IRIOrLiteral") => Some(NodeKind::IriOrLiteral),
        x if x == shacl("BlankNodeOrLiteral") => Some(NodeKind::BlankNodeOrLiteral),
        _ => None,
    }
}

/// Resolve an RDF collection starting at `start` into its members.
fn collect_list(
    dict: &dyn DictionaryCodec,
    map: &HashMap<String, Vec<(String, Term)>>,
    start: &Term,
) -> Vec<Term> {
    let mut out = Vec::new();
    let mut cur = start.clone();
    let mut seen = BTreeSet::new();
    loop {
        let key = term_key(dict, &cur);
        if !seen.insert(key.clone()) {
            break;
        }
        let Some(triples) = map.get(&key) else { break };
        let first = triples
            .iter()
            .find(|(p, _)| p == RDF_FIRST)
            .map(|(_, o)| o.clone());
        let rest = triples
            .iter()
            .find(|(p, _)| p == RDF_REST)
            .map(|(_, o)| o.clone());
        match (first, rest) {
            (Some(f), Some(r)) => {
                out.push(f);
                cur = r;
            }
            (Some(f), None) => {
                out.push(f);
                break;
            }
            _ => break,
        }
    }
    out
}

fn parse_property_shape(
    dict: &dyn DictionaryCodec,
    map: &HashMap<String, Vec<(String, Term)>>,
    key: &str,
) -> PropertyShape {
    let Some(triples) = map.get(key) else {
        return PropertyShape {
            id: key.to_owned(),
            path: String::new(),
            constraints: Vec::new(),
            property_shapes: Vec::new(),
            severity: Severity::Violation,
            message: None,
        };
    };
    let path = triples
        .iter()
        .find(|(p, _)| p == &shacl("path"))
        .and_then(|(_, o)| term_iri(o))
        .unwrap_or_default();
    let (mut constraints, severity, message) = parse_shape_body(dict, map, triples);
    let mut property_shapes = Vec::new();
    // Property-shape only parameters (must not appear on node shapes).
    for (p, o) in triples {
        match p.as_str() {
            x if x == shacl("qualifiedValueShape") => {
                constraints.push(ConstraintComponent::QualifiedValueShape {
                    shape: term_key(dict, o),
                });
            }
            x if x == shacl("qualifiedMinCount") => {
                if let Some(n) = literal_usize(o) {
                    constraints.push(ConstraintComponent::QualifiedMinCount(n));
                }
            }
            x if x == shacl("qualifiedMaxCount") => {
                if let Some(n) = literal_usize(o) {
                    constraints.push(ConstraintComponent::QualifiedMaxCount(n));
                }
            }
            x if x == shacl("qualifiedValueShapesDisjoint") && literal_bool(o) == Some(true) => {
                constraints.push(ConstraintComponent::QualifiedValueShapesDisjoint);
            }
            x if x == shacl("property") => {
                let nested = term_key(dict, o);
                property_shapes.push(parse_property_shape(dict, map, &nested));
            }
            _ => {}
        }
    }
    PropertyShape {
        id: key.to_owned(),
        path,
        constraints,
        property_shapes,
        severity,
        message,
    }
}

/// Parse constraints / severity / message from one shape body.
/// Returns `(constraints, severity, message)`.
fn parse_shape_body(
    dict: &dyn DictionaryCodec,
    map: &HashMap<String, Vec<(String, Term)>>,
    triples: &[(String, Term)],
) -> (Vec<ConstraintComponent>, Severity, Option<String>) {
    let mut constraints = Vec::new();
    let mut severity = Severity::Violation;
    let mut message = None;
    for (p, o) in triples {
        match p.as_str() {
            x if x == shacl("class") => {
                if let Some(iri) = term_iri(o) {
                    constraints.push(ConstraintComponent::Class(iri));
                }
            }
            x if x == shacl("datatype") => {
                if let Some(iri) = term_iri(o) {
                    constraints.push(ConstraintComponent::Datatype(iri));
                }
            }
            x if x == shacl("nodeKind") => {
                if let Some(kind) = parse_node_kind(dict, o) {
                    constraints.push(ConstraintComponent::NodeKind(kind));
                }
            }
            x if x == shacl("minCount") => {
                if let Some(n) = literal_usize(o) {
                    constraints.push(ConstraintComponent::MinCount(n));
                }
            }
            x if x == shacl("maxCount") => {
                if let Some(n) = literal_usize(o) {
                    constraints.push(ConstraintComponent::MaxCount(n));
                }
            }
            x if x == shacl("minInclusive") => {
                constraints.push(ConstraintComponent::MinInclusive(o.clone()));
            }
            x if x == shacl("maxInclusive") => {
                constraints.push(ConstraintComponent::MaxInclusive(o.clone()));
            }
            x if x == shacl("minExclusive") => {
                constraints.push(ConstraintComponent::MinExclusive(o.clone()));
            }
            x if x == shacl("maxExclusive") => {
                constraints.push(ConstraintComponent::MaxExclusive(o.clone()));
            }
            x if x == shacl("minLength") => {
                if let Some(n) = literal_usize(o) {
                    constraints.push(ConstraintComponent::MinLength(n));
                }
            }
            x if x == shacl("maxLength") => {
                if let Some(n) = literal_usize(o) {
                    constraints.push(ConstraintComponent::MaxLength(n));
                }
            }
            x if x == shacl("pattern") => {
                if let Some(s) = literal_string(o) {
                    constraints.push(ConstraintComponent::Pattern(s));
                }
            }
            x if x == shacl("in") => {
                let values = collect_list(dict, map, o);
                if !values.is_empty() {
                    constraints.push(ConstraintComponent::In(values));
                }
            }
            x if x == shacl("hasValue") => {
                constraints.push(ConstraintComponent::HasValue(o.clone()));
            }
            x if x == shacl("languageIn") => {
                let tags: Vec<String> = collect_list(dict, map, o)
                    .into_iter()
                    .filter_map(|t| literal_string(&t))
                    .collect();
                if !tags.is_empty() {
                    constraints.push(ConstraintComponent::LanguageIn(tags));
                }
            }
            x if x == shacl("uniqueLang") => {
                if literal_bool(o) == Some(true) {
                    constraints.push(ConstraintComponent::UniqueLang);
                }
            }
            x if x == shacl("node") => {
                constraints.push(ConstraintComponent::Node(term_key(dict, o)));
            }
            x if x == shacl("and") => {
                let shapes_list: Vec<String> = collect_list(dict, map, o)
                    .into_iter()
                    .map(|t| term_key(dict, &t))
                    .collect();
                if !shapes_list.is_empty() {
                    constraints.push(ConstraintComponent::And(shapes_list));
                }
            }
            x if x == shacl("or") => {
                let shapes_list: Vec<String> = collect_list(dict, map, o)
                    .into_iter()
                    .map(|t| term_key(dict, &t))
                    .collect();
                if !shapes_list.is_empty() {
                    constraints.push(ConstraintComponent::Or(shapes_list));
                }
            }
            x if x == shacl("xone") => {
                let shapes_list: Vec<String> = collect_list(dict, map, o)
                    .into_iter()
                    .map(|t| term_key(dict, &t))
                    .collect();
                if !shapes_list.is_empty() {
                    constraints.push(ConstraintComponent::Xone(shapes_list));
                }
            }
            x if x == shacl("not") => {
                constraints.push(ConstraintComponent::Not(term_key(dict, o)));
            }
            // Property-pair constraints are valid on node shapes too: on a node
            // shape the focus node itself is the value set being compared.
            x if x == shacl("equals") => {
                if let Some(iri) = term_iri(o) {
                    constraints.push(ConstraintComponent::Equals(iri));
                }
            }
            x if x == shacl("disjoint") => {
                if let Some(iri) = term_iri(o) {
                    constraints.push(ConstraintComponent::Disjoint(iri));
                }
            }
            x if x == shacl("lessThan") => {
                if let Some(iri) = term_iri(o) {
                    constraints.push(ConstraintComponent::LessThan(iri));
                }
            }
            x if x == shacl("lessThanOrEquals") => {
                if let Some(iri) = term_iri(o) {
                    constraints.push(ConstraintComponent::LessThanOrEquals(iri));
                }
            }
            x if x == shacl("flags") => {
                if let Some(s) = literal_string(o) {
                    constraints.push(ConstraintComponent::PatternFlags(s));
                }
            }
            x if x == shacl("closed") => {
                if literal_bool(o) == Some(true) {
                    constraints.push(ConstraintComponent::Closed);
                }
            }
            x if x == shacl("severity") => {
                if let Some(s) = parse_severity(o) {
                    severity = s;
                }
            }
            x if x == shacl("message") => {
                if let Some(s) = literal_string(o) {
                    message = Some(s);
                }
            }
            _ => {}
        }
    }
    (constraints, severity, message)
}

fn parse_shapes(dict: &dyn DictionaryCodec, shapes: &[Triple]) -> Vec<Shape> {
    let map = index_triples(dict, shapes);
    let mut out = Vec::new();
    for (id, triples) in &map {
        if !is_shacl_body(triples) {
            continue;
        }
        let mut targets = Vec::new();
        let mut property_shapes = Vec::new();
        let mut ignored_properties = Vec::new();
        let mut deactivated = false;
        let (constraints, severity, message) = parse_shape_body(dict, &map, triples);
        // A shape that declares `sh:path` is a standalone property shape: its
        // own constraints apply to the values of the path.
        let path = triples
            .iter()
            .find(|(p, _)| p == &shacl("path"))
            .and_then(|(_, o)| term_iri(o));
        for (p, o) in triples {
            match p.as_str() {
                x if x == shacl("targetClass") => {
                    if let Some(iri) = term_iri(o) {
                        targets.push(Target::Class(iri));
                    }
                }
                // Implicit class target: a shape that is itself a class
                // (`rdfs:Class` / `owl:Class`) targets all its instances.
                x if x == RDF_TYPE
                    && matches!(
                        term_iri(o).as_deref(),
                        Some(RDFS_CLASS) | Some(OWL_CLASS)
                    ) =>
                {
                    targets.push(Target::Class(id.clone()));
                }
                x if x == shacl("targetNode") => targets.push(Target::Node(o.clone())),
                x if x == shacl("targetSubjectsOf") => {
                    if let Some(iri) = term_iri(o) {
                        targets.push(Target::SubjectsOf(iri));
                    }
                }
                x if x == shacl("targetObjectsOf") => {
                    if let Some(iri) = term_iri(o) {
                        targets.push(Target::ObjectsOf(iri));
                    }
                }
                x if x == shacl("property") => {
                    let key = term_key(dict, o);
                    property_shapes.push(parse_property_shape(dict, &map, &key));
                }
                x if x == shacl("ignoredProperties") => {
                    ignored_properties = collect_list(dict, &map, o)
                        .into_iter()
                        .filter_map(|t| term_iri(&t))
                        .collect();
                }
                x if x == shacl("deactivated") && literal_bool(o) == Some(true) => {
                    deactivated = true;
                }
                _ => {}
            }
        }
        out.push(Shape {
            id: id.clone(),
            targets,
            path,
            constraints,
            property_shapes,
            ignored_properties,
            deactivated,
            severity,
            message,
        });
    }
    out
}

fn select_targets(dict: &dyn DictionaryCodec, data: &[Triple], shape: &Shape) -> Vec<String> {
    let mut targets = BTreeSet::new();
    for target in &shape.targets {
        match target {
            Target::Node(n) => {
                targets.insert(term_key(dict, n));
            }
            Target::Class(class) => {
                for t in data {
                    let node = subject_key(dict, t.subject);
                    if is_instance_of(dict, data, &node, class) {
                        targets.insert(node);
                    }
                }
            }
            Target::SubjectsOf(p) => {
                for t in data {
                    if t.predicate.as_str() == p {
                        targets.insert(subject_key(dict, t.subject));
                    }
                }
            }
            Target::ObjectsOf(p) => {
                for t in data {
                    if t.predicate.as_str() == p {
                        targets.insert(term_key(dict, &t.object));
                    }
                }
            }
        }
    }
    targets.into_iter().collect()
}

fn is_instance_of(dict: &dyn DictionaryCodec, data: &[Triple], node: &str, class: &str) -> bool {
    // SHACL "instance of" follows `rdfs:subClassOf` chains: walk from the
    // required class down through its (transitive) subclasses and check the
    // node's direct `rdf:type` against each.
    let direct = |c: &str| {
        data.iter().any(|t| {
            subject_key(dict, t.subject) == node
                && t.predicate.as_str() == RDF_TYPE
                && term_key(dict, &t.object) == c
        })
    };
    if direct(class) {
        return true;
    }
    let mut frontier = vec![class.to_owned()];
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while let Some(c) = frontier.pop() {
        if !seen.insert(c.clone()) {
            continue;
        }
        for t in data {
            if t.predicate.as_str() != RDFS_SUBCLASS_OF || term_key(dict, &t.object) != c {
                continue;
            }
            let subclass = subject_key(dict, t.subject);
            if direct(&subclass) {
                return true;
            }
            frontier.push(subclass);
        }
    }
    false
}

/// All `(key, term)` value nodes of `path` for the focus node.
fn path_values(
    dict: &dyn DictionaryCodec,
    data: &[Triple],
    focus: &str,
    path: &str,
) -> Vec<(String, Term)> {
    data.iter()
        .filter(|t| subject_key(dict, t.subject) == focus && t.predicate.as_str() == path)
        .map(|t| (term_key(dict, &t.object), t.object.clone()))
        .collect()
}

fn default_message(component: &str, detail: &str) -> String {
    match component {
        c if c == shacl("ClassConstraintComponent") => {
            format!("value is not an instance of {detail}")
        }
        c if c == shacl("DatatypeConstraintComponent") => {
            format!("value is not a literal with datatype {detail}")
        }
        c if c == shacl("NodeKindConstraintComponent") => {
            format!("value does not have node kind {detail}")
        }
        c if c == shacl("MinCountConstraintComponent") => {
            format!("fewer than {detail} values")
        }
        c if c == shacl("MaxCountConstraintComponent") => {
            format!("more than {detail} values")
        }
        c if c == shacl("MinLengthConstraintComponent") => {
            format!("string shorter than {detail} characters")
        }
        c if c == shacl("MaxLengthConstraintComponent") => {
            format!("string longer than {detail} characters")
        }
        c if c == shacl("PatternConstraintComponent") => {
            format!("string does not match pattern /{detail}/")
        }
        c if c == shacl("InConstraintComponent") => "value not in allowed list".to_owned(),
        c if c == shacl("HasValueConstraintComponent") => "required value not present".to_owned(),
        c if c == shacl("LanguageInConstraintComponent") => {
            "value is not a literal with an allowed language tag".to_owned()
        }
        c if c == shacl("UniqueLangConstraintComponent") => {
            "multiple values share the same language tag".to_owned()
        }
        c if c == shacl("NodeConstraintComponent") => "node does not conform".to_owned(),
        c if c == shacl("AndConstraintComponent") => {
            "value does not conform to all shapes in sh:and".to_owned()
        }
        c if c == shacl("OrConstraintComponent") => {
            "value does not conform to any shape in sh:or".to_owned()
        }
        c if c == shacl("XoneConstraintComponent") => {
            "value does not conform to exactly one shape in sh:xone".to_owned()
        }
        c if c == shacl("NotConstraintComponent") => {
            "value conforms to the shape in sh:not".to_owned()
        }
        c if c == shacl("MinInclusiveConstraintComponent") => {
            "value is less than the inclusive minimum".to_owned()
        }
        c if c == shacl("MaxInclusiveConstraintComponent") => {
            "value is greater than the inclusive maximum".to_owned()
        }
        c if c == shacl("MinExclusiveConstraintComponent") => {
            "value is not greater than the exclusive minimum".to_owned()
        }
        c if c == shacl("MaxExclusiveConstraintComponent") => {
            "value is not less than the exclusive maximum".to_owned()
        }
        c if c == shacl("EqualsConstraintComponent") => {
            "value set does not equal the value set of the given property".to_owned()
        }
        c if c == shacl("DisjointConstraintComponent") => {
            "value set is not disjoint from the given property".to_owned()
        }
        c if c == shacl("LessThanConstraintComponent") => {
            "value is not less than all values of the given property".to_owned()
        }
        c if c == shacl("LessThanOrEqualsConstraintComponent") => {
            "value is not less than or equal to all values of the given property".to_owned()
        }
        c if c == shacl("QualifiedMinCountConstraintComponent") => {
            format!("fewer than {detail} values conform to the qualified value shape")
        }
        c if c == shacl("QualifiedMaxCountConstraintComponent") => {
            format!("more than {detail} values conform to the qualified value shape")
        }
        c if c == shacl("ClosedConstraintComponent") => {
            "property not allowed by closed shape".to_owned()
        }
        _ => "constraint violated".to_owned(),
    }
}

#[allow(clippy::too_many_arguments)]
fn push_result(
    results: &mut Vec<ValidationResult>,
    focus: &str,
    path: Option<String>,
    value: Option<String>,
    source_shape: Option<&str>,
    component: &str,
    detail: &str,
    severity: &Severity,
    message: Option<&str>,
) {
    results.push(ValidationResult {
        focus_node: focus.to_owned(),
        path,
        value,
        source_shape: source_shape.map(str::to_owned),
        component: component.to_owned(),
        severity: severity.clone(),
        message: Some(
            message
                .map(str::to_owned)
                .unwrap_or_else(|| default_message(component, detail)),
        ),
    });
}

#[allow(clippy::too_many_arguments)]
fn check_values(
    dict: &dyn DictionaryCodec,
    data: &[Triple],
    shapes: &[Shape],
    focus: &str,
    path: Option<&str>,
    values: &[(String, Term)],
    constraint: &ConstraintComponent,
    source_shape: Option<&str>,
    ps: Option<&PropertyShape>,
    severity: &Severity,
    message: Option<&str>,
    results: &mut Vec<ValidationResult>,
    depth: usize,
) {
    match constraint {
        ConstraintComponent::Class(class) => {
            for (vkey, _) in values {
                if !is_instance_of(dict, data, vkey, class) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("ClassConstraintComponent"),
                        class,
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::Datatype(dt) => {
            for (vkey, v) in values {
                let ok = matches!(v, Term::Literal(lv) if lv.xsd_datatype_iri().as_str() == dt
                    && lexical_valid_for_datatype(dt, &lv.lexical_form()));
                if !ok {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("DatatypeConstraintComponent"),
                        dt,
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::NodeKind(kind) => {
            for (vkey, v) in values {
                let ok = matches!(
                    (kind, v),
                    (NodeKind::Iri, Term::Iri(_))
                        | (NodeKind::BlankNode, Term::BlankNode(_))
                        | (NodeKind::Literal, Term::Literal(_))
                        | (NodeKind::BlankNodeOrIri, Term::BlankNode(_) | Term::Iri(_))
                        | (NodeKind::IriOrLiteral, Term::Iri(_) | Term::Literal(_))
                        | (
                            NodeKind::BlankNodeOrLiteral,
                            Term::BlankNode(_) | Term::Literal(_)
                        )
                );
                if !ok {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("NodeKindConstraintComponent"),
                        kind_name(kind),
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::MinLength(n) => {
            for (vkey, v) in values {
                // Per SHACL, `str(v)` of IRIs and literals participates; blank
                // nodes always fail.
                let len = term_str_len(v);
                if !len.is_some_and(|l| l >= *n) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("MinLengthConstraintComponent"),
                        &n.to_string(),
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::MaxLength(n) => {
            for (vkey, v) in values {
                let len = term_str_len(v);
                if !len.is_some_and(|l| l <= *n) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("MaxLengthConstraintComponent"),
                        &n.to_string(),
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::Pattern(pat) => {
            let flags = sibling_pattern_flags(shapes, source_shape, ps);
            for (vkey, v) in values {
                let ok = match v {
                    // `str(v)` participates for IRIs and literals; blank nodes
                    // have no string representation and always fail.
                    Term::BlankNode(_) => false,
                    Term::Iri(iri) => pattern_matches_flags(pat, iri.as_str(), &flags),
                    Term::Literal(lv) => pattern_matches_flags(pat, &lv.lexical_form(), &flags),
                };
                if !ok {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("PatternConstraintComponent"),
                        pat,
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::MinInclusive(limit) => {
            for (vkey, v) in values {
                if !value_ge(v, limit) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("MinInclusiveConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::MaxInclusive(limit) => {
            for (vkey, v) in values {
                if !value_le(v, limit) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("MaxInclusiveConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::MinExclusive(limit) => {
            for (vkey, v) in values {
                if !value_gt(v, limit) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("MinExclusiveConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::MaxExclusive(limit) => {
            for (vkey, v) in values {
                if !value_lt(v, limit) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("MaxExclusiveConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::Equals(other_path) => {
            let other = path_values(dict, data, focus, other_path);
            for (vkey, _) in values {
                if !other.iter().any(|(k, _)| k == vkey) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("EqualsConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
            for (ukey, _) in &other {
                if !values.iter().any(|(k, _)| k == ukey) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(ukey.clone()),
                        source_shape,
                        &shacl("EqualsConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::Disjoint(other_path) => {
            let other = path_values(dict, data, focus, other_path);
            for (vkey, _) in values {
                if other.iter().any(|(k, _)| k == vkey) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("DisjointConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::LessThan(other_path) => {
            let other = path_values(dict, data, focus, other_path);
            for (vkey, v) in values {
                let ok = other
                    .iter()
                    .all(|(_, u)| compare_terms(v, u) == Some(Ordering::Less));
                if !ok {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("LessThanConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::LessThanOrEquals(other_path) => {
            let other = path_values(dict, data, focus, other_path);
            for (vkey, v) in values {
                let ok = other.iter().all(|(_, u)| {
                    matches!(
                        compare_terms(v, u),
                        Some(Ordering::Less) | Some(Ordering::Equal)
                    )
                });
                if !ok {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("LessThanOrEqualsConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::In(allowed) => {
            let allowed_keys: Vec<String> = allowed.iter().map(|t| term_key(dict, t)).collect();
            for (vkey, _) in values {
                if !allowed_keys.contains(vkey) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("InConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::HasValue(required) => {
            let wanted = term_key(dict, required);
            let found = values.iter().any(|(k, _)| *k == wanted);
            if !found {
                // W3C suite convention (data-shapes#111): sh:hasValue results
                // carry no sh:value.
                push_result(
                    results,
                    focus,
                    path.map(str::to_owned),
                    None,
                    source_shape,
                    &shacl("HasValueConstraintComponent"),
                    "",
                    severity,
                    message,
                );
            }
        }
        ConstraintComponent::LanguageIn(allowed) => {
            for (vkey, v) in values {
                let ok = matches!(v, Term::Literal(lv) if lv.language_tag().is_some_and(|tag| {
                    // Basic language range matching (RFC 4647): a tag matches
                    // the range when it equals the range or extends it with a
                    // subtag, e.g. "en-NZ" matches "en".
                    let tag = tag.as_str().to_ascii_lowercase();
                    allowed.iter().any(|a| {
                        let a = a.to_ascii_lowercase();
                        tag == a || tag.strip_prefix(&a).is_some_and(|rest| rest.starts_with('-'))
                    })
                }));
                if !ok {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("LanguageInConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::UniqueLang => {
            // One result per focus node when any pair of value nodes shares a
            // language tag; the result carries no sh:value (W3C suite).
            let mut seen: Vec<String> = Vec::new();
            let mut duplicate = false;
            for (_, v) in values {
                if let Term::Literal(lv) = v
                    && let Some(tag) = lv.language_tag()
                {
                    if seen.iter().any(|s| s.eq_ignore_ascii_case(tag.as_str())) {
                        duplicate = true;
                        break;
                    }
                    seen.push(tag.as_str().to_owned());
                }
            }
            if duplicate {
                push_result(
                    results,
                    focus,
                    path.map(str::to_owned),
                    None,
                    source_shape,
                    &shacl("UniqueLangConstraintComponent"),
                    "",
                    severity,
                    message,
                );
            }
        }
        ConstraintComponent::Node(shape_key) => {
            for (vkey, _) in values {
                // A single NodeConstraintComponent result per non-conforming
                // value node (W3C suite), rather than the inner results.
                if !conforms_to(dict, data, shapes, shape_key, vkey, depth) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("NodeConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::And(shape_keys) => {
            for (vkey, _) in values {
                let conforms = shape_keys
                    .iter()
                    .all(|k| conforms_to(dict, data, shapes, k, vkey, depth));
                if !conforms {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("AndConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::Or(shape_keys) => {
            for (vkey, _) in values {
                let conforms = shape_keys
                    .iter()
                    .any(|k| conforms_to(dict, data, shapes, k, vkey, depth));
                if !conforms {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("OrConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::Xone(shape_keys) => {
            for (vkey, _) in values {
                let conforming = shape_keys
                    .iter()
                    .filter(|k| conforms_to(dict, data, shapes, k, vkey, depth))
                    .count();
                if conforming != 1 {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("XoneConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::Not(shape_key) => {
            for (vkey, _) in values {
                if conforms_to(dict, data, shapes, shape_key, vkey, depth) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("NotConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::QualifiedValueShape { .. }
        | ConstraintComponent::QualifiedValueShapesDisjoint
        | ConstraintComponent::PatternFlags(_) => {}
        ConstraintComponent::QualifiedMinCount(n) => {
            if let Some((shape_key, disjoint, siblings)) =
                qualified_value_context(shapes, ps)
            {
                let count = values
                    .iter()
                    .filter(|(vkey, _)| {
                        let mut matches = conforms_to(dict, data, shapes, &shape_key, vkey, depth);
                        if matches && disjoint {
                            matches = !siblings
                                .iter()
                                .any(|s| conforms_to(dict, data, shapes, s, vkey, depth));
                        }
                        matches
                    })
                    .count();
                if count < *n {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        None,
                        source_shape,
                        &shacl("QualifiedMinCountConstraintComponent"),
                        &n.to_string(),
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::QualifiedMaxCount(n) => {
            if let Some((shape_key, disjoint, siblings)) =
                qualified_value_context(shapes, ps)
            {
                let count = values
                    .iter()
                    .filter(|(vkey, _)| {
                        let mut matches = conforms_to(dict, data, shapes, &shape_key, vkey, depth);
                        if matches && disjoint {
                            matches = !siblings
                                .iter()
                                .any(|s| conforms_to(dict, data, shapes, s, vkey, depth));
                        }
                        matches
                    })
                    .count();
                if count > *n {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        None,
                        source_shape,
                        &shacl("QualifiedMaxCountConstraintComponent"),
                        &n.to_string(),
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::MinCount(n) => {
            if values.len() < *n {
                push_result(
                    results,
                    focus,
                    path.map(str::to_owned),
                    None,
                    source_shape,
                    &shacl("MinCountConstraintComponent"),
                    &n.to_string(),
                    severity,
                    message,
                );
            }
        }
        ConstraintComponent::MaxCount(n) => {
            if values.len() > *n {
                push_result(
                    results,
                    focus,
                    path.map(str::to_owned),
                    None,
                    source_shape,
                    &shacl("MaxCountConstraintComponent"),
                    &n.to_string(),
                    severity,
                    message,
                );
            }
        }
        ConstraintComponent::Closed => {
            let allowed: Vec<String> = shapes
                .iter()
                .find(|s| s.id == *source_shape.unwrap_or(""))
                .map(|s| {
                    let mut paths: Vec<String> =
                        s.property_shapes.iter().map(|ps| ps.path.clone()).collect();
                    paths.extend(s.ignored_properties.iter().cloned());
                    paths
                })
                .unwrap_or_default();
            for t in data {
                if subject_key(dict, t.subject) == focus
                    && !allowed.iter().any(|p| p == t.predicate.as_str())
                {
                    push_result(
                        results,
                        focus,
                        Some(t.predicate.as_str().to_owned()),
                        Some(term_key(dict, &t.object)),
                        source_shape,
                        &shacl("ClosedConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
    }
}

fn kind_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Iri => "IRI",
        NodeKind::BlankNode => "BlankNode",
        NodeKind::Literal => "Literal",
        NodeKind::BlankNodeOrIri => "BlankNodeOrIRI",
        NodeKind::IriOrLiteral => "IRIOrLiteral",
        NodeKind::BlankNodeOrLiteral => "BlankNodeOrLiteral",
    }
}

/// Look up the `sh:qualifiedValueShape` parameter of the current property shape.
/// Returns `(shape_key, sh:qualifiedValueShapesDisjoint, sibling_qualified_shapes)`.
fn qualified_value_context<'a>(
    shapes: &'a [Shape],
    ps: Option<&'a PropertyShape>,
) -> Option<(String, bool, Vec<&'a str>)> {
    let ps = ps?;
    // Sibling qualified shapes live on the node shape that owns this property
    // shape (its `sh:property` list), not on the property shape itself.
    let shape = shapes.iter().find(|s| {
        s.property_shapes
            .iter()
            .any(|other| std::ptr::eq(other, ps))
    })?;
    let shape_key = ps.constraints.iter().find_map(|c| match c {
        ConstraintComponent::QualifiedValueShape { shape } => Some(shape.clone()),
        _ => None,
    })?;
    let disjoint = ps
        .constraints
        .iter()
        .any(|c| matches!(c, ConstraintComponent::QualifiedValueShapesDisjoint));
    let siblings: Vec<&str> = shape
        .property_shapes
        .iter()
        .filter(|other| !std::ptr::eq(*other, ps))
        .filter_map(|other| {
            other.constraints.iter().find_map(|c| match c {
                ConstraintComponent::QualifiedValueShape { shape } => Some(shape.as_str()),
                _ => None,
            })
        })
        .collect();
    Some((shape_key, disjoint, siblings))
}

/// Regex flags of a sibling `sh:flags` in the same shape body (property shape
/// if given, otherwise the node shape that carries the `sh:pattern`).
fn sibling_pattern_flags(
    shapes: &[Shape],
    source_shape: Option<&str>,
    ps: Option<&PropertyShape>,
) -> String {
    let flags = match ps {
        Some(ps) => ps.constraints.iter().find_map(|c| match c {
            ConstraintComponent::PatternFlags(f) => Some(f.clone()),
            _ => None,
        }),
        None => shapes
            .iter()
            .find(|s| s.id == source_shape.unwrap_or(""))
            .and_then(|s| {
                s.constraints.iter().find_map(|c| match c {
                    ConstraintComponent::PatternFlags(f) => Some(f.clone()),
                    _ => None,
                })
            }),
    };
    flags.unwrap_or_default()
}

/// True if evaluating `shape_key` with focus node `focus` produces no `sh:Violation` results.
fn conforms_to(
    dict: &dyn DictionaryCodec,
    data: &[Triple],
    shapes: &[Shape],
    shape_key: &str,
    focus: &str,
    depth: usize,
) -> bool {
    if depth > 32 {
        return true;
    }
    let mut local = Vec::new();
    evaluate_shape(dict, data, shapes, shape_key, focus, &mut local, depth + 1);
    local.iter().all(|r| r.severity != Severity::Violation)
}

fn evaluate_shape(
    dict: &dyn DictionaryCodec,
    data: &[Triple],
    shapes: &[Shape],
    shape_key: &str,
    focus: &str,
    results: &mut Vec<ValidationResult>,
    depth: usize,
) {
    if depth > 32 {
        return;
    }
    let Some(shape) = shapes.iter().find(|s| s.id == shape_key) else {
        return;
    };
    if shape.deactivated {
        return;
    }
    if let Some(path) = &shape.path {
        // Standalone property shape: constraints apply to values of the path.
        let values = path_values(dict, data, focus, path);
        for c in &shape.constraints {
            check_values(
                dict,
                data,
                shapes,
                focus,
                Some(path),
                &values,
                c,
                Some(&shape.id),
                None,
                &shape.severity,
                shape.message.as_deref(),
                results,
                depth,
            );
        }
        for ps in &shape.property_shapes {
            for (vkey, _) in &values {
                evaluate_property_shape(dict, data, shapes, ps, vkey, results, depth);
            }
        }
    } else {
        // Node-shape constraints apply to the focus node itself.
        let focus_values = vec![(focus.to_owned(), focus_as_term(focus))];
        for c in &shape.constraints {
            check_values(
                dict,
                data,
                shapes,
                focus,
                None,
                &focus_values,
                c,
                Some(&shape.id),
                None,
                &shape.severity,
                shape.message.as_deref(),
                results,
                depth,
            );
        }
        // Property shapes apply to values of the path.
        for ps in &shape.property_shapes {
            evaluate_property_shape(dict, data, shapes, ps, focus, results, depth);
        }
    }
}

/// Evaluate one property shape with the given focus node: its constraints
/// apply to the values of its path, and nested `sh:property` shapes apply to
/// each value node as a new focus.
fn evaluate_property_shape(
    dict: &dyn DictionaryCodec,
    data: &[Triple],
    shapes: &[Shape],
    ps: &PropertyShape,
    focus: &str,
    results: &mut Vec<ValidationResult>,
    depth: usize,
) {
    if depth > 32 {
        return;
    }
    let values = path_values(dict, data, focus, &ps.path);
    for c in &ps.constraints {
        check_values(
            dict,
            data,
            shapes,
            focus,
            Some(&ps.path),
            &values,
            c,
            Some(&ps.id),
            Some(ps),
            &ps.severity,
            ps.message.as_deref(),
            results,
            depth,
        );
    }
    for nested in &ps.property_shapes {
        for (vkey, _) in &values {
            evaluate_property_shape(dict, data, shapes, nested, vkey, results, depth);
        }
    }
}

/// Materialize a focus node key back to a `Term` for constraint evaluation.
fn focus_as_term(key: &str) -> Term {
    if let Some(rest) = key.strip_prefix("literal:") {
        // Formats: `lex|datatype` or `lex|rdf:langString|tag`.
        if let Some((head, tail)) = rest.rsplit_once('|') {
            if let Some(lex) = head.strip_suffix(RDF_LANG_STRING).and_then(|h| h.strip_suffix('|'))
            {
                let lang = LanguageTag::parse(tail)
                    .unwrap_or_else(|_| LanguageTag::parse("und").expect("und is a valid tag"));
                return Term::Literal(LiteralValue::Lang {
                    value: lex.to_owned(),
                    lang,
                });
            }
            let lex = head;
            let dt = tail;
            if dt == "http://www.w3.org/2001/XMLSchema#integer"
                && let Ok(n) = lex.parse::<i64>()
            {
                return Term::Literal(LiteralValue::Integer(n));
            }
            if dt == "http://www.w3.org/2001/XMLSchema#decimal"
                && let Ok(f) = lex.parse::<f64>()
            {
                return Term::Literal(LiteralValue::Decimal(f));
            }
            if dt == "http://www.w3.org/2001/XMLSchema#float"
                && let Ok(f) = lex.parse::<f32>()
            {
                return Term::Literal(LiteralValue::Float(f));
            }
            if dt == "http://www.w3.org/2001/XMLSchema#double"
                && let Ok(f) = lex.parse::<f64>()
            {
                return Term::Literal(LiteralValue::Double(f));
            }
            if dt == "http://www.w3.org/2001/XMLSchema#boolean"
                && let Ok(b) = lex.parse::<bool>()
            {
                return Term::Literal(LiteralValue::Boolean(b));
            }
            if dt == "http://www.w3.org/2001/XMLSchema#string" {
                return Term::Literal(LiteralValue::String(lex.to_owned()));
            }
            // Other typed literals round-trip lexically.
            return Term::Literal(LiteralValue::Typed {
                value: lex.to_owned(),
                datatype: Iri::new(dt),
            });
        }
        return Term::Literal(LiteralValue::String(rest.to_owned()));
    }
    // Blank-node labels (`_:...`) materialize as blank nodes; the exact id is
    // irrelevant for constraint evaluation (only node kind/str-ness matter).
    if key.starts_with("_:") {
        return Term::BlankNode(NodeId::new(0));
    }
    Term::Iri(Iri::new(key))
}

impl ShaclValidator for ShaclEngine {
    fn validate(
        &self,
        dict: &dyn DictionaryCodec,
        shapes: &[Triple],
        data: &[Triple],
    ) -> Result<ValidationReport, OntolithError> {
        let shapes = parse_shapes(dict, shapes);
        let mut results = Vec::new();
        for shape in &shapes {
            for focus in select_targets(dict, data, shape) {
                evaluate_shape(dict, data, &shapes, &shape.id, &focus, &mut results, 0);
            }
        }
        results.sort_by(|a, b| {
            a.focus_node
                .cmp(&b.focus_node)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.value.cmp(&b.value))
        });
        // W3C SHACL: a data graph conforms to a shapes graph only when no
        // validation results at all are produced (warnings and infos included).
        let conforms = results.is_empty();
        Ok(ValidationReport { conforms, results })
    }
}

// ---------------------------------------------------------------------------
// Minimal regex subset (SPARQL `regex()` search semantics; no groups/
// alternation). `^`/`$` anchor the match; `*+?` and `{n,m}` quantifiers are
// supported.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Atom {
    Literal(char),
    Any,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    Digit,
    Word,
    Space,
    NonDigit,
    NonWord,
    NonSpace,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Quant {
    Star,
    Plus,
    Question,
    Exactly(usize),
    AtLeast(usize),
    Range(usize, usize),
}

fn parse_atom(chars: &[char], i: &mut usize) -> Option<Atom> {
    let c = chars[*i];
    *i += 1;
    match c {
        '.' => Some(Atom::Any),
        '\\' => {
            let e = *chars.get(*i)?;
            *i += 1;
            Some(match e {
                'd' => Atom::Digit,
                'w' => Atom::Word,
                's' => Atom::Space,
                'D' => Atom::NonDigit,
                'W' => Atom::NonWord,
                'S' => Atom::NonSpace,
                other => Atom::Literal(other),
            })
        }
        '[' => {
            let mut negated = false;
            if chars.get(*i) == Some(&'^') {
                negated = true;
                *i += 1;
            }
            let mut ranges = Vec::new();
            loop {
                let lo = *chars.get(*i)?;
                *i += 1;
                if lo == ']' {
                    break;
                }
                if lo == '\\' {
                    let e = *chars.get(*i)?;
                    *i += 1;
                    let hi = if chars.get(*i) == Some(&'-')
                        && chars.get(*i + 1).is_some_and(|n| *n != ']')
                    {
                        *i += 1;
                        let h = *chars.get(*i)?;
                        *i += 1;
                        Some(h)
                    } else {
                        None
                    };
                    ranges.push((e, hi.unwrap_or(e)));
                    continue;
                }
                if chars.get(*i) == Some(&'-') && chars.get(*i + 1).is_some_and(|n| *n != ']') {
                    let hi = *chars.get(*i + 1)?;
                    *i += 2;
                    ranges.push((lo, hi));
                } else {
                    ranges.push((lo, lo));
                }
            }
            Some(Atom::Class { negated, ranges })
        }
        other => Some(Atom::Literal(other)),
    }
}

fn parse_pattern(pattern: &str) -> (Vec<(Atom, Option<Quant>)>, bool, bool) {
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    let mut tokens = Vec::new();
    let anchored_start = chars.first() == Some(&'^');
    let anchored_end = chars.last() == Some(&'$');
    while i < chars.len() {
        if chars[i] == '^' {
            i += 1;
            continue;
        }
        if chars[i] == '$' {
            i += 1;
            continue;
        }
        let atom = parse_atom(&chars, &mut i).unwrap_or(Atom::Literal(chars[i - 1]));
        let quant = match chars.get(i) {
            Some('*') => {
                i += 1;
                Some(Quant::Star)
            }
            Some('+') => {
                i += 1;
                Some(Quant::Plus)
            }
            Some('?') => {
                i += 1;
                Some(Quant::Question)
            }
            Some('{') if i + 1 < chars.len() => parse_brace_quant(&chars, &mut i),
            _ => None,
        };
        tokens.push((atom, quant));
    }
    (tokens, anchored_start, anchored_end)
}

/// Parse a `{n}` / `{n,}` / `{n,m}` quantifier after an atom. On success the
/// cursor is advanced past the closing brace; on failure the `{` stays literal.
fn parse_brace_quant(chars: &[char], i: &mut usize) -> Option<Quant> {
    let start = *i;
    let rest = &chars[*i + 1..];
    let close = rest.iter().position(|c| *c == '}')?;
    let body: String = rest[..close].iter().collect();
    let q = match body.split_once(',') {
        None => Quant::Exactly(body.trim().parse::<usize>().ok()?),
        Some((lo, hi)) if hi.trim().is_empty() => {
            Quant::AtLeast(lo.trim().parse::<usize>().ok()?)
        }
        Some((lo, hi)) => {
            let l = lo.trim().parse::<usize>().ok()?;
            let h = hi.trim().parse::<usize>().ok()?;
            if l > h {
                return None;
            }
            Quant::Range(l, h)
        }
    };
    *i = start + close + 2;
    Some(q)
}

fn atom_matches(atom: &Atom, c: char) -> bool {
    match atom {
        Atom::Literal(l) => *l == c,
        Atom::Any => true,
        Atom::Digit => c.is_ascii_digit(),
        Atom::Word => c.is_ascii_alphanumeric() || c == '_',
        Atom::Space => c.is_ascii_whitespace(),
        Atom::NonDigit => !c.is_ascii_digit(),
        Atom::NonWord => !(c.is_ascii_alphanumeric() || c == '_'),
        Atom::NonSpace => !c.is_ascii_whitespace(),
        Atom::Class { negated, ranges } => {
            let hit = ranges.iter().any(|(lo, hi)| *lo <= c && c <= *hi);
            hit != *negated
        }
    }
}

fn match_at(
    tokens: &[(Atom, Option<Quant>)],
    ti: usize,
    text: &[char],
    ci: usize,
    anchored_end: bool,
) -> bool {
    if ti == tokens.len() {
        return !anchored_end || ci == text.len();
    }
    let (atom, quant) = &tokens[ti];
    match quant {
        None => {
            if ci < text.len() && atom_matches(atom, text[ci]) {
                match_at(tokens, ti + 1, text, ci + 1, anchored_end)
            } else {
                false
            }
        }
        Some(Quant::Question) => {
            match_at(tokens, ti + 1, text, ci, anchored_end)
                || (ci < text.len()
                    && atom_matches(atom, text[ci])
                    && match_at(tokens, ti + 1, text, ci + 1, anchored_end))
        }
        Some(Quant::Star) => {
            match_at(tokens, ti + 1, text, ci, anchored_end)
                || (ci < text.len()
                    && atom_matches(atom, text[ci])
                    && match_at(tokens, ti, text, ci + 1, anchored_end))
        }
        Some(Quant::Plus) => {
            ci < text.len()
                && atom_matches(atom, text[ci])
                && (match_at(tokens, ti + 1, text, ci + 1, anchored_end)
                    || match_at(tokens, ti, text, ci + 1, anchored_end))
        }
        Some(Quant::Exactly(n)) => {
            let end = ci + *n;
            if end <= text.len()
                && text[ci..end].iter().all(|c| atom_matches(atom, *c))
            {
                match_at(tokens, ti + 1, text, end, anchored_end)
            } else {
                false
            }
        }
        Some(Quant::AtLeast(n)) => {
            let mut consumed = 0;
            while consumed < *n
                && ci + consumed < text.len()
                && atom_matches(atom, text[ci + consumed])
            {
                consumed += 1;
            }
            if consumed < *n {
                false
            } else {
                at_least_rest(tokens, ti + 1, text, ci + consumed, anchored_end, atom, None)
            }
        }
        Some(Quant::Range(lo, hi)) => {
            let mut consumed = 0;
            while consumed < *lo
                && ci + consumed < text.len()
                && atom_matches(atom, text[ci + consumed])
            {
                consumed += 1;
            }
            if consumed < *lo {
                false
            } else {
                at_least_rest(
                    tokens,
                    ti + 1,
                    text,
                    ci + consumed,
                    anchored_end,
                    atom,
                    Some(*hi - *lo),
                )
            }
        }
    }
}

/// Try to continue matching after a `{n,}` / `{n,m}` atom consumed its minimum,
/// greedily consuming up to `remaining` extra matches (None = unbounded).
fn at_least_rest(
    tokens: &[(Atom, Option<Quant>)],
    ti: usize,
    text: &[char],
    ci: usize,
    anchored_end: bool,
    atom: &Atom,
    remaining: Option<usize>,
) -> bool {
    if match_at(tokens, ti, text, ci, anchored_end) {
        return true;
    }
    if remaining == Some(0) || ci >= text.len() || !atom_matches(atom, text[ci]) {
        return false;
    }
    at_least_rest(
        tokens,
        ti,
        text,
        ci + 1,
        anchored_end,
        atom,
        remaining.map(|r| r - 1),
    )
}

fn pattern_matches(pattern: &str, text: &str) -> bool {
    let (tokens, anchored_start, anchored_end) = parse_pattern(pattern);
    let text: Vec<char> = text.chars().collect();
    if anchored_start {
        match_at(&tokens, 0, &text, 0, anchored_end)
    } else {
        (0..=text.len()).any(|start| match_at(&tokens, 0, &text, start, anchored_end))
    }
}

/// `sh:pattern` with optional regex flags (`i` = case-insensitive; other flags ignored).
fn pattern_matches_flags(pattern: &str, text: &str, flags: &str) -> bool {
    if flags.contains('i') {
        pattern_matches(&pattern.to_lowercase(), &text.to_lowercase())
    } else {
        pattern_matches(pattern, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_parser::infrastructure::parse_turtle_doc;
    use ontolith_storage::infrastructure::InMemoryDictionary;

    fn load_turtle(dict: &InMemoryDictionary, src: &str) -> Vec<Triple> {
        parse_turtle_doc(src, dict)
            .expect("turtle parse")
            .dataset
            .default_graph
    }

    fn validate(dict: &InMemoryDictionary, shapes_src: &str, data_src: &str) -> ValidationReport {
        let shapes = load_turtle(dict, shapes_src);
        let data = load_turtle(dict, data_src);
        ShaclEngine::new()
            .validate(dict, &shapes, &data)
            .expect("validation")
    }

    const SH: &str = "PREFIX sh: <http://www.w3.org/ns/shacl#> PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> PREFIX ex: <http://ex.org/> ";

    #[test]
    fn class_constraint_violation() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ; sh:class ex:Adult ."
            ),
            &format!(
                "{SH}
                ex:alice a ex:Person, ex:Adult . ex:bob a ex:Person ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/bob");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#ClassConstraintComponent"
        );
    }

    #[test]
    fn conforming_data_passes() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ; sh:class ex:Adult ."
            ),
            &format!(
                "{SH}
                ex:alice a ex:Person, ex:Adult . ex:bob a ex:Person, ex:Adult ."
            ),
        );
        assert!(report.conforms);
        assert!(report.results.is_empty());
    }

    #[test]
    fn property_min_count_and_datatype() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:OrderShape a sh:NodeShape ; sh:targetClass ex:Order ;
                    sh:property [ sh:path ex:status ; sh:minCount 1 ; sh:datatype xsd:string ;
                                  sh:message \"status required as string\" ] ."
            ),
            &format!(
                "{SH}
                ex:o1 a ex:Order ; ex:status \"open\" . ex:o2 a ex:Order ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/o2");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#MinCountConstraintComponent"
        );
        assert_eq!(
            report.results[0].message.as_deref(),
            Some("status required as string")
        );
    }

    #[test]
    fn pattern_and_min_length() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:UserShape a sh:NodeShape ; sh:targetClass ex:User ;
                    sh:property [ sh:path ex:email ; sh:nodeKind sh:Literal ;
                                  sh:pattern \"^[a-z]+@[a-z]+\\\\.[a-z]+$\" ; sh:minLength 5 ] ."
            ),
            &format!(
                "{SH}
                ex:u1 a ex:User ; ex:email \"alice@example.com\" .
                ex:u2 a ex:User ; ex:email \"BAD\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        assert!(
            report
                .results
                .iter()
                .all(|r| r.focus_node == "http://ex.org/u2")
        );
        let components: Vec<_> = report
            .results
            .iter()
            .map(|r| r.component.as_str())
            .collect();
        assert!(components.contains(&"http://www.w3.org/ns/shacl#PatternConstraintComponent"));
        assert!(components.contains(&"http://www.w3.org/ns/shacl#MinLengthConstraintComponent"));
    }

    #[test]
    fn closed_shape_rejects_extra_property() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:StrictShape a sh:NodeShape ; sh:targetClass ex:Strict ; sh:closed true ;
                    sh:ignoredProperties ( rdf:type ) ;
                    sh:property [ sh:path ex:name ; sh:minCount 1 ] ."
            ),
            &format!(
                "{SH}
                ex:s1 a ex:Strict ; ex:name \"ok\" .
                ex:s2 a ex:Strict ; ex:name \"x\" ; ex:extra 1 ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(
            report.results[0].path.as_deref(),
            Some("http://ex.org/extra")
        );
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#ClosedConstraintComponent"
        );
    }

    #[test]
    fn in_constraint_rejects_unknown_value() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TaskShape a sh:NodeShape ; sh:targetClass ex:Task ;
                    sh:property [ sh:path ex:status ; sh:in ( \"open\" \"closed\" ) ] ."
            ),
            &format!(
                "{SH}
                ex:t1 a ex:Task ; ex:status \"open\" .
                ex:t2 a ex:Task ; ex:status \"bogus\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/t2");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#InConstraintComponent"
        );
    }

    #[test]
    fn node_reference_recursion() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:AddressShape a sh:NodeShape ;
                    sh:property [ sh:path ex:city ; sh:minCount 1 ] .
                ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
                    sh:property [ sh:path ex:address ; sh:node ex:AddressShape ] ."
            ),
            &format!(
                "{SH}
                ex:p1 a ex:Person ; ex:address ex:a1 . ex:a1 ex:city \"Paris\" .
                ex:p2 a ex:Person ; ex:address ex:a2 . ex:a2 ex:zip \"75000\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/p2");
        assert_eq!(report.results[0].path.as_deref(), Some("http://ex.org/address"));
        assert_eq!(report.results[0].value.as_deref(), Some("http://ex.org/a2"));
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#NodeConstraintComponent"
        );
    }

    #[test]
    fn target_subjects_of() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:OwnedShape a sh:NodeShape ; sh:targetSubjectsOf ex:owner ; sh:class ex:Owned ."
            ),
            &format!(
                "{SH}
                ex:car1 ex:owner ex:alice ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/car1");
    }

    #[test]
    fn severity_warning_still_breaks_conformance() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:NoteShape a sh:NodeShape ; sh:targetClass ex:Note ;
                    sh:property [ sh:path ex:summary ; sh:minCount 1 ;
                                  sh:severity sh:Warning ] ."
            ),
            &format!(
                "{SH}
                ex:n1 a ex:Note ."
            ),
        );
        // W3C SHACL: conforms = no validation results at all (warnings count).
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].severity, Severity::Warning);
    }

    #[test]
    fn and_constraint_fails_when_one_shape_fails() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:AdultShape a sh:NodeShape ; sh:class ex:Adult .
                ex:NamedShape a sh:NodeShape ;
                    sh:property [ sh:path ex:name ; sh:minCount 1 ] .
                ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
                    sh:and ( ex:AdultShape ex:NamedShape ) ."
            ),
            &format!(
                "{SH}
                ex:alice a ex:Person, ex:Adult ; ex:name \"Alice\" .
                ex:bob a ex:Person, ex:Adult .
                ex:carol a ex:Person ; ex:name \"Carol\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        assert!(
            report
                .results
                .iter()
                .all(|r| r.component == "http://www.w3.org/ns/shacl#AndConstraintComponent")
        );
        let foci: Vec<_> = report
            .results
            .iter()
            .map(|r| r.focus_node.as_str())
            .collect();
        assert!(foci.contains(&"http://ex.org/bob"));
        assert!(foci.contains(&"http://ex.org/carol"));
    }

    #[test]
    fn or_constraint_passes_when_any_shape_matches() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:HasEmailShape a sh:NodeShape ;
                    sh:property [ sh:path ex:email ; sh:minCount 1 ] .
                ex:HasPhoneShape a sh:NodeShape ;
                    sh:property [ sh:path ex:phone ; sh:minCount 1 ] .
                ex:ContactShape a sh:NodeShape ; sh:targetClass ex:Contact ;
                    sh:or ( ex:HasEmailShape ex:HasPhoneShape ) ."
            ),
            &format!(
                "{SH}
                ex:c1 a ex:Contact ; ex:email \"a@x.org\" .
                ex:c2 a ex:Contact ; ex:phone \"+86\" .
                ex:c3 a ex:Contact ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/c3");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#OrConstraintComponent"
        );
    }

    #[test]
    fn not_constraint_violates_when_shape_matches() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:BannedStatusShape a sh:NodeShape ; sh:hasValue \"banned\" .
                ex:ItemShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:property [ sh:path ex:status ; sh:not ex:BannedStatusShape ] ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item ; ex:status \"active\" .
                ex:i2 a ex:Item ; ex:status \"banned\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/i2");
        assert_eq!(
            report.results[0].path.as_deref(),
            Some("http://ex.org/status")
        );
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#NotConstraintComponent"
        );
    }

    #[test]
    fn qualified_min_count_enforces_min_matches() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TeamShape a sh:NodeShape ; sh:targetClass ex:Team ;
                    sh:property [ sh:path ex:member ;
                                  sh:qualifiedValueShape [ sh:class ex:Engineer ] ;
                                  sh:qualifiedMinCount 2 ] ."
            ),
            &format!(
                "{SH}
                ex:t1 a ex:Team ; ex:member ex:e1, ex:e2 .
                ex:e1 a ex:Engineer . ex:e2 a ex:Engineer .
                ex:t2 a ex:Team ; ex:member ex:e3 .
                ex:e3 a ex:Engineer .
                ex:t3 a ex:Team ; ex:member ex:e4, ex:e5 .
                ex:e4 a ex:Engineer . ex:e5 a ex:Manager ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        assert!(
            report.results.iter().all(|r| r.component
                == "http://www.w3.org/ns/shacl#QualifiedMinCountConstraintComponent")
        );
        let foci: Vec<_> = report
            .results
            .iter()
            .map(|r| r.focus_node.as_str())
            .collect();
        assert!(foci.contains(&"http://ex.org/t2"));
        assert!(foci.contains(&"http://ex.org/t3"));
    }

    #[test]
    fn qualified_max_count_enforces_max_matches() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TeamShape a sh:NodeShape ; sh:targetClass ex:Team ;
                    sh:property [ sh:path ex:member ;
                                  sh:qualifiedValueShape [ sh:class ex:Engineer ] ;
                                  sh:qualifiedMaxCount 1 ] ."
            ),
            &format!(
                "{SH}
                ex:t1 a ex:Team ; ex:member ex:e1 . ex:e1 a ex:Engineer .
                ex:t2 a ex:Team ; ex:member ex:e1, ex:e2 .
                ex:e1 a ex:Engineer . ex:e2 a ex:Engineer ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/t2");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#QualifiedMaxCountConstraintComponent"
        );
    }

    #[test]
    fn qualified_value_shapes_disjoint_excludes_sibling_matches() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TeamShape a sh:NodeShape ; sh:targetClass ex:Team ;
                    sh:property [ sh:path ex:member ;
                                  sh:qualifiedValueShape [ sh:class ex:Engineer ] ;
                                  sh:qualifiedValueShapesDisjoint true ;
                                  sh:qualifiedMinCount 1 ] ;
                    sh:property [ sh:path ex:member ;
                                  sh:qualifiedValueShape [ sh:class ex:Lead ] ;
                                  sh:qualifiedMaxCount 0 ] ."
            ),
            &format!(
                "{SH}
                ex:t1 a ex:Team ; ex:member ex:l1 .
                ex:l1 a ex:Lead, ex:Engineer ."
            ),
        );
        // l1 matches both sibling qualified shapes. It is excluded from the
        // disjoint engineer count (0 < qualifiedMinCount 1) and still counts
        // against the lead max (1 > qualifiedMaxCount 0).
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        let components: Vec<_> = report
            .results
            .iter()
            .map(|r| r.component.as_str())
            .collect();
        assert!(
            components.contains(&"http://www.w3.org/ns/shacl#QualifiedMinCountConstraintComponent")
        );
        assert!(
            components.contains(&"http://www.w3.org/ns/shacl#QualifiedMaxCountConstraintComponent")
        );
        assert!(
            report
                .results
                .iter()
                .all(|r| r.focus_node == "http://ex.org/t1")
        );
    }

    #[test]
    fn closed_shape_allows_ignored_properties() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:StrictShape a sh:NodeShape ; sh:targetClass ex:Strict ;
                    sh:closed true ; sh:ignoredProperties ( ex:comment rdf:type ) ;
                    sh:property [ sh:path ex:name ; sh:minCount 1 ] ."
            ),
            &format!(
                "{SH}
                ex:s1 a ex:Strict ; ex:name \"ok\" ; ex:comment \"note\" .
                ex:s2 a ex:Strict ; ex:name \"x\" ; ex:extra 1 ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(
            report.results[0].path.as_deref(),
            Some("http://ex.org/extra")
        );
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#ClosedConstraintComponent"
        );
    }

    #[test]
    fn numeric_range_constraints() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
                    sh:property [ sh:path ex:age ; sh:minInclusive 18 ; sh:maxInclusive 65 ] ;
                    sh:property [ sh:path ex:score ; sh:minExclusive 0 ; sh:maxExclusive 100 ] ."
            ),
            &format!(
                "{SH}
                ex:p1 a ex:Person ; ex:age 30 ; ex:score 80 .
                ex:p2 a ex:Person ; ex:age 17 ; ex:score 50 .
                ex:p3 a ex:Person ; ex:age 70 ; ex:score 50 .
                ex:p4 a ex:Person ; ex:age 30 ; ex:score 0 .
                ex:p5 a ex:Person ; ex:age 30 ; ex:score 100 ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 4);
        let components: Vec<_> = report
            .results
            .iter()
            .map(|r| r.component.as_str())
            .collect();
        assert!(components.contains(&"http://www.w3.org/ns/shacl#MinInclusiveConstraintComponent"));
        assert!(components.contains(&"http://www.w3.org/ns/shacl#MaxInclusiveConstraintComponent"));
        assert!(components.contains(&"http://www.w3.org/ns/shacl#MinExclusiveConstraintComponent"));
        assert!(components.contains(&"http://www.w3.org/ns/shacl#MaxExclusiveConstraintComponent"));
        let foci: Vec<_> = report
            .results
            .iter()
            .map(|r| r.focus_node.as_str())
            .collect();
        assert!(foci.contains(&"http://ex.org/p2"));
        assert!(foci.contains(&"http://ex.org/p3"));
        assert!(foci.contains(&"http://ex.org/p4"));
        assert!(foci.contains(&"http://ex.org/p5"));
    }

    #[test]
    fn equals_constraint_requires_same_value_set() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
                    sh:property [ sh:path ex:primaryEmail ; sh:equals ex:email ] ."
            ),
            &format!(
                "{SH}
                ex:p1 a ex:Person ; ex:primaryEmail \"a@x.org\" ; ex:email \"a@x.org\" .
                ex:p2 a ex:Person ; ex:primaryEmail \"a@x.org\" ; ex:email \"b@x.org\" .
                ex:p3 a ex:Person ; ex:primaryEmail \"a@x.org\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 3);
        assert!(
            report
                .results
                .iter()
                .all(|r| r.component == "http://www.w3.org/ns/shacl#EqualsConstraintComponent")
        );
        assert_eq!(
            report
                .results
                .iter()
                .filter(|r| r.focus_node == "http://ex.org/p2")
                .count(),
            2
        );
        assert_eq!(
            report
                .results
                .iter()
                .filter(|r| r.focus_node == "http://ex.org/p3")
                .count(),
            1
        );
    }

    #[test]
    fn disjoint_constraint_rejects_overlap() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:StaffShape a sh:NodeShape ; sh:targetClass ex:Staff ;
                    sh:property [ sh:path ex:teaches ; sh:disjoint ex:studies ] ."
            ),
            &format!(
                "{SH}
                ex:s1 a ex:Staff ; ex:teaches ex:c1 ; ex:studies ex:c2 .
                ex:s2 a ex:Staff ; ex:teaches ex:c1, ex:c2 ; ex:studies ex:c2 ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/s2");
        assert_eq!(report.results[0].value.as_deref(), Some("http://ex.org/c2"));
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#DisjointConstraintComponent"
        );
    }

    #[test]
    fn less_than_constraints_order_values() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:EventShape a sh:NodeShape ; sh:targetClass ex:Event ;
                    sh:property [ sh:path ex:start ; sh:lessThan ex:end ] ;
                    sh:property [ sh:path ex:end ; sh:lessThanOrEquals ex:deadline ] ."
            ),
            &format!(
                "{SH}
                ex:e1 a ex:Event ; ex:start 1 ; ex:end 2 ; ex:deadline 2 .
                ex:e2 a ex:Event ; ex:start 3 ; ex:end 2 ; ex:deadline 2 .
                ex:e3 a ex:Event ; ex:start 1 ; ex:end 2 ; ex:deadline 1 ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        assert_eq!(report.results[0].focus_node, "http://ex.org/e2");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#LessThanConstraintComponent"
        );
        assert_eq!(report.results[1].focus_node, "http://ex.org/e3");
        assert_eq!(
            report.results[1].component,
            "http://www.w3.org/ns/shacl#LessThanOrEqualsConstraintComponent"
        );
    }

    #[test]
    fn pattern_flags_case_insensitive() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:CodeShape a sh:NodeShape ; sh:targetClass ex:Code ;
                    sh:property [ sh:path ex:code ; sh:pattern \"^[a-z]+$\" ; sh:flags \"i\" ] ."
            ),
            &format!(
                "{SH}
                ex:c1 a ex:Code ; ex:code \"HELLO\" .
                ex:c2 a ex:Code ; ex:code \"Hello123\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/c2");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#PatternConstraintComponent"
        );
    }

    #[test]
    fn language_in_accepts_allowed_tags() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:LabelShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:property [ sh:path ex:label ; sh:languageIn (\"en\" \"fr\") ] ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item ; ex:label \"hello\"@en .
                ex:i2 a ex:Item ; ex:label \"bonjour\"@fr ."
            ),
        );
        assert!(report.conforms, "both labels use allowed tags: {:?}", report.results);
        assert!(report.results.is_empty());
    }

    #[test]
    fn language_in_rejects_other_tags_and_non_literals() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:LabelShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:property [ sh:path ex:label ; sh:languageIn (\"en\" \"fr\") ] ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item ; ex:label \"hallo\"@de .
                ex:i2 a ex:Item ; ex:label \"plain\" .
                ex:i3 a ex:Item ; ex:label ex:iriValue ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 3);
        assert!(
            report
                .results
                .iter()
                .all(|r| r.component == "http://www.w3.org/ns/shacl#LanguageInConstraintComponent")
        );
    }

    #[test]
    fn language_in_compares_tags_case_insensitively() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:LabelShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:property [ sh:path ex:label ; sh:languageIn (\"en\") ] ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item ; ex:label \"hello\"@EN ."
            ),
        );
        assert!(report.conforms, "EN should match en (case-insensitive): {:?}", report.results);
    }

    #[test]
    fn unique_lang_rejects_duplicate_tags() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TitleShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:property [ sh:path ex:title ; sh:uniqueLang true ] ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item ; ex:title \"hello\"@en, \"hi\"@EN .
                ex:i2 a ex:Item ; ex:title \"hello\"@en, \"salut\"@fr ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/i1");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#UniqueLangConstraintComponent"
        );
    }

    #[test]
    fn unique_lang_ignores_non_lang_values() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TitleShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:property [ sh:path ex:title ; sh:uniqueLang true ] ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item ; ex:title \"plain\" ; ex:title \"hello\"@en .
                ex:i2 a ex:Item ; ex:title \"hello\"@en, \"salut\"@fr ."
            ),
        );
        assert!(report.conforms, "non-tagged literals do not compete: {:?}", report.results);
    }

    #[test]
    fn lang_literal_equality_distinguishes_tags() {
        let dict = InMemoryDictionary::new();
        // sh:in/hasValue with language-tagged literals: tags participate in equality.
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:GreetingShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:property [ sh:path ex:greeting ; sh:in (\"hello\"@en) ] ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item ; ex:greeting \"hello\"@en .
                ex:i2 a ex:Item ; ex:greeting \"hello\"@fr ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/i2");
    }

    #[test]
    fn string_constraints_apply_to_lang_literals() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:LabelShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:property [ sh:path ex:label ; sh:minLength 5 ; sh:pattern \"^[a-z]+$\" ] ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item ; ex:label \"hello\"@en .
                ex:i2 a ex:Item ; ex:label \"hi\"@en .
                ex:i3 a ex:Item ; ex:label \"Hello!\"@en ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        assert!(
            report
                .results
                .iter()
                .any(|r| r.focus_node == "http://ex.org/i2"
                    && r.component == "http://www.w3.org/ns/shacl#MinLengthConstraintComponent")
        );
        assert!(
            report
                .results
                .iter()
                .any(|r| r.focus_node == "http://ex.org/i3"
                    && r.component == "http://www.w3.org/ns/shacl#PatternConstraintComponent")
        );
    }

    #[test]
    fn xone_requires_exactly_one_matching_shape() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:AFamily a sh:NodeShape ; sh:class ex:A .
                ex:BFamily a sh:NodeShape ; sh:class ex:B .
                ex:CategoryShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:xone ( ex:AFamily ex:BFamily ) ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item, ex:A .
                ex:i2 a ex:Item, ex:A, ex:B .
                ex:i3 a ex:Item ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        assert!(
            report
                .results
                .iter()
                .all(|r| r.component == "http://www.w3.org/ns/shacl#XoneConstraintComponent")
        );
        let foci: Vec<_> = report
            .results
            .iter()
            .map(|r| r.focus_node.as_str())
            .collect();
        assert!(foci.contains(&"http://ex.org/i2"));
        assert!(foci.contains(&"http://ex.org/i3"));
    }

    #[test]
    fn xone_applies_to_property_values() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:IntShape a sh:NodeShape ; sh:datatype xsd:integer .
                ex:SmallShape a sh:NodeShape ; sh:maxInclusive 10 .
                ex:AmountShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:property [ sh:path ex:amount ; sh:xone ( ex:IntShape ex:SmallShape ) ] ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item ; ex:amount 5 .
                ex:i2 a ex:Item ; ex:amount 50 .
                ex:i3 a ex:Item ; ex:amount \"abc\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        let foci: Vec<_> = report
            .results
            .iter()
            .map(|r| r.focus_node.as_str())
            .collect();
        assert!(foci.contains(&"http://ex.org/i1"));
        assert!(foci.contains(&"http://ex.org/i3"));
    }

    #[test]
    fn custom_severity_iri_round_trips() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TestShape a sh:NodeShape ; sh:targetNode ex:bad ;
                    sh:datatype xsd:boolean ; sh:severity ex:MySeverity ."
            ),
            &format!("{SH} ex:bad ex:p 1 ."),
        );
        assert!(!report.conforms);
        assert_eq!(
            report.results[0].severity,
            Severity::Custom("http://ex.org/MySeverity".to_owned())
        );
        assert_eq!(
            report.results[0].severity.clone().iri(),
            "http://ex.org/MySeverity"
        );
    }

    #[test]
    fn date_time_range_constraints_compare_instants() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TestShape a sh:NodeShape ;
                    sh:minInclusive \"2002-10-10T12:00:00-05:00\"^^xsd:dateTime ;
                    sh:targetNode \"2002-10-10T12:00:01-05:00\"^^xsd:dateTime ;
                    sh:targetNode \"2002-10-10T12:00:00-05:00\"^^xsd:dateTime ;
                    sh:targetNode \"2002-10-09T12:00:00-05:00\"^^xsd:dateTime ;
                    sh:targetNode \"2002-10-10T12:00:00\"^^xsd:dateTime ."
            ),
            &format!("{SH} ex:unused ex:p 1 ."),
        );
        assert!(!report.conforms);
        // Two violations: the earlier instant and the timezone-less literal
        // (not comparable against a timezone-carrying minimum).
        assert_eq!(report.results.len(), 2);
        assert!(
            report
                .results
                .iter()
                .all(|r| r.component
                    == "http://www.w3.org/ns/shacl#MinInclusiveConstraintComponent")
        );
    }

    #[test]
    fn nested_property_shapes_reach_shared_sub_shapes() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:s1 a sh:NodeShape ; sh:targetNode ex:i ;
                    sh:property ex:s2 ; sh:property ex:s3 .
                ex:s2 sh:path ex:p ; sh:property ex:s4 .
                ex:s3 sh:path ex:q ; sh:property ex:s4 .
                ex:s4 sh:path ex:r ; sh:class ex:C ."
            ),
            &format!(
                "{SH}
                ex:i ex:p ex:j . ex:i ex:q ex:j . ex:j ex:r ex:k ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        assert!(
            report
                .results
                .iter()
                .all(|r| r.focus_node == "http://ex.org/j"
                    && r.path.as_deref() == Some("http://ex.org/r")
                    && r.component == "http://www.w3.org/ns/shacl#ClassConstraintComponent")
        );
    }

    #[test]
    fn datatype_constraint_rejects_ill_formed_lexicals() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TestShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:property [ sh:path ex:value ; sh:datatype xsd:integer ] ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item ; ex:value 42 .
                ex:i2 a ex:Item ; ex:value \"aldi\"^^xsd:integer .
                ex:i3 a ex:Item ; ex:value \"55\"^^xsd:integer ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(
            report.results[0].value.as_deref(),
            Some("literal:aldi|http://www.w3.org/2001/XMLSchema#integer")
        );
    }

    #[test]
    fn language_in_matches_basic_language_ranges() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:LabelShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:property [ sh:path ex:label ; sh:languageIn (\"en\" \"mi\") ] ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item ; ex:label \"Hill\"@en-NZ .
                ex:i2 a ex:Item ; ex:label \"Maunga\"@mi .
                ex:i3 a ex:Item ; ex:label \"Berg\"@de ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/i3");
    }
}
