//! RDF syntax serialization.
//!
//! Writers for N-Triples / N-Quads (line-oriented), Turtle / TriG
//! (block-oriented), and a pragmatic RDF/XML writer. Subject/blank-node
//! [`NodeId`]s are decoded through an optional [`DictionaryCodec`] so stored
//! IRI subjects survive export; without a dictionary every subject is treated
//! as a blank node (`_:n<id>`), which preserves the historical behaviour of
//! the line-oriented writers.

use ontolith_core::domain::{Iri, LiteralValue, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Dataset, Quad, Term, Triple};
use ontolith_storage::application::DictionaryCodec;

/// Supported RDF serialization formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerializeFormat {
    NTriples,
    NQuads,
    Turtle,
    TriG,
}

impl SerializeFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NTriples => "n-triples",
            Self::NQuads => "n-quads",
            Self::Turtle => "turtle",
            Self::TriG => "trig",
        }
    }
}

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema#";

/// Render a single triple as an N-Triples line (no trailing newline).
pub fn serialize_triple(triple: &Triple) -> String {
    format!(
        "{} {} {} .",
        render_subject(triple.subject, None),
        render_iri(&triple.predicate),
        render_term(&triple.object, None)
    )
}

/// Render a single quad as an N-Quads line (no trailing newline).
pub fn serialize_quad(quad: &Quad) -> String {
    let triple = serialize_triple(&quad.triple);
    match &quad.graph_name {
        Some(graph) => format!(
            "{} <{}> .",
            triple.trim_end_matches(" ."),
            render_iri_text(graph)
        ),
        None => triple,
    }
}

/// Serialize a dataset in a line- or block-oriented format. Without a
/// dictionary, subjects render as blank nodes (`_:n<id>`).
pub fn serialize_dataset(dataset: &Dataset, format: SerializeFormat) -> String {
    serialize_dataset_with(dataset, format, None)
}

/// Dict-aware dataset serialization (see module docs).
pub fn serialize_dataset_with(
    dataset: &Dataset,
    format: SerializeFormat,
    dict: Option<&dyn DictionaryCodec>,
) -> String {
    let mut out = String::new();
    match format {
        SerializeFormat::TriG => serialize_trig(dataset, dict, &mut out),
        SerializeFormat::NTriples | SerializeFormat::NQuads | SerializeFormat::Turtle => {
            for quad in dataset.quads() {
                let line = match format {
                    SerializeFormat::NTriples | SerializeFormat::Turtle
                        if quad.graph_name.is_some() =>
                    {
                        continue;
                    }
                    SerializeFormat::NTriples | SerializeFormat::Turtle => {
                        serialize_triple_with_dict(&quad.triple, dict)
                    }
                    SerializeFormat::NQuads => serialize_quad_with_dict(&quad, dict),
                    _ => unreachable!(),
                };
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    out
}

fn serialize_quad_with_dict(quad: &Quad, dict: Option<&dyn DictionaryCodec>) -> String {
    let triple = format!(
        "{} {} {} .",
        render_subject(quad.triple.subject, dict),
        render_iri(&quad.triple.predicate),
        render_term(&quad.triple.object, dict)
    );
    match &quad.graph_name {
        Some(graph) => format!(
            "{} <{}> .",
            triple.trim_end_matches(" ."),
            render_iri_text(graph)
        ),
        None => triple,
    }
}

/// TriG: default-graph triples at top level, named graphs as `<g> { ... }`.
fn serialize_trig(dataset: &Dataset, dict: Option<&dyn DictionaryCodec>, out: &mut String) {
    for quad in dataset.quads() {
        if quad.graph_name.is_none() {
            out.push_str(&serialize_triple_with_dict(&quad.triple, dict));
            out.push('\n');
        }
    }
    for graph in &dataset.named_graphs {
        out.push('<');
        out.push_str(&render_iri_text(&graph.name));
        out.push_str("> {\n");
        for triple in &graph.triples {
            out.push('\t');
            out.push_str(&serialize_triple_with_dict(triple, dict));
            out.push('\n');
        }
        out.push_str("}\n");
    }
}

fn serialize_triple_with_dict(triple: &Triple, dict: Option<&dyn DictionaryCodec>) -> String {
    format!(
        "{} {} {} .",
        render_subject(triple.subject, dict),
        render_iri(&triple.predicate),
        render_term(&triple.object, dict)
    )
}

/// Pragmatic RDF/XML writer for the default graph.
///
/// Each subject becomes one `<rdf:Description rdf:about="…">` (or
/// `rdf:nodeID`) whose children are property elements. Predicate IRIs are
/// emitted as QNames: the IRI is split at its last `/`, `#` or `:` and the
/// namespace is declared with a generated prefix. Predicates whose local part
/// is not an XML NCName (and other constructions RDF/XML cannot express)
/// produce a deterministic error.
pub fn serialize_rdf_xml(
    dataset: &Dataset,
    dict: Option<&dyn DictionaryCodec>,
) -> Result<String, OntolithError> {
    // Index default-graph triples by subject (deterministic ordering).
    let mut by_subject: Vec<(String, Triple)> = Vec::new();
    for quad in dataset.quads() {
        if quad.graph_name.is_some() {
            continue;
        }
        let key = subject_key(quad.triple.subject, dict);
        by_subject.push((key, quad.triple));
    }
    by_subject.sort_by(|a, b| a.0.cmp(&b.0));

    // Build predicate → qname; register namespaces in first-use order.
    let mut namespace_order: Vec<String> = Vec::new();
    let mut predicate_qname: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    for (_, t) in &by_subject {
        if !predicate_qname.contains_key(t.predicate.as_str()) {
            let (ns, local) = split_ns_local(t.predicate.as_str(), dict)?;
            if !namespace_order.contains(&ns) {
                namespace_order.push(ns.clone());
            }
            predicate_qname.insert(
                t.predicate.as_str().to_owned(),
                (namespace_prefix(&ns, &namespace_order), local),
            );
        }
    }

    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<rdf:RDF xmlns:rdf=\"");
    out.push_str(RDF_NS);
    out.push('"');
    for (i, ns) in namespace_order.iter().enumerate() {
        out.push_str(" xmlns:ns");
        out.push_str(&i.to_string());
        out.push_str("=\"");
        xml_escape_attr(ns, &mut out);
        out.push('"');
    }
    out.push_str(">\n");

    let mut idx = 0;
    while idx < by_subject.len() {
        let (key, first) = &by_subject[idx];
        let subject_node = first.subject;
        write_subject_start(&mut out, subject_node, dict, key);
        out.push_str(">\n");
        while idx < by_subject.len() && by_subject[idx].0 == *key {
            let triple = &by_subject[idx].1;
            write_property_element(&mut out, triple, dict, &predicate_qname);
            idx += 1;
        }
        out.push_str("</rdf:Description>\n");
    }
    out.push_str("</rdf:RDF>\n");
    Ok(out)
}

fn write_subject_start(
    out: &mut String,
    subject: NodeId,
    dict: Option<&dyn DictionaryCodec>,
    key: &str,
) {
    out.push_str("  <rdf:Description ");
    if key.starts_with("_:") {
        out.push_str("rdf:nodeID=\"");
        xml_escape_attr(subject_label(subject, dict).as_str(), out);
    } else {
        out.push_str("rdf:about=\"");
        xml_escape_attr(key, out);
    }
    out.push('"');
}

fn write_property_element(
    out: &mut String,
    triple: &Triple,
    dict: Option<&dyn DictionaryCodec>,
    qnames: &std::collections::HashMap<String, (String, String)>,
) {
    let (prefix, local) = &qnames[triple.predicate.as_str()];
    out.push_str("    <");
    out.push_str(prefix);
    out.push(':');
    out.push_str(local);
    match &triple.object {
        Term::Iri(obj) => {
            out.push_str(" rdf:resource=\"");
            xml_escape_attr(obj.as_str(), out);
            out.push_str("\"/>\n");
        }
        Term::BlankNode(id) => {
            out.push_str(" rdf:nodeID=\"");
            xml_escape_attr(subject_label(*id, dict).as_str(), out);
            out.push_str("\"/>\n");
        }
        Term::Literal(lit) => {
            let lex = lit.lexical_form();
            if let Some(lang) = lit.language_tag() {
                out.push_str(" xml:lang=\"");
                xml_escape_attr(lang.as_str(), out);
                out.push_str("\">");
                xml_escape_text(&lex, out);
                out.push_str("</");
                out.push_str(prefix);
                out.push(':');
                out.push_str(local);
                out.push_str(">\n");
            } else {
                let dt = lit.xsd_datatype_iri();
                if dt.as_str() != format!("{XSD_NS}string") {
                    out.push_str(" rdf:datatype=\"");
                    xml_escape_attr(dt.as_str(), out);
                    out.push_str("\">");
                } else {
                    out.push('>');
                }
                xml_escape_text(&lex, out);
                out.push_str("</");
                out.push_str(prefix);
                out.push(':');
                out.push_str(local);
                out.push_str(">\n");
            }
        }
    }
}

/// Subject key: `_:label` for blank-node subjects, otherwise the IRI text.
fn subject_key(subject: NodeId, dict: Option<&dyn DictionaryCodec>) -> String {
    if let Some(label) = decode_blank_label(subject, dict) {
        format!("_:{label}")
    } else if let Some(dict) = dict
        && let Some(v) = dict.decode_node(subject)
        && !v.starts_with("_:")
    {
        v
    } else {
        format!("_:n{}", subject.get())
    }
}

/// Decode a blank-node label for `NodeId`; `None` when the node is an IRI or
/// no dictionary is available.
fn decode_blank_label(subject: NodeId, dict: Option<&dyn DictionaryCodec>) -> Option<String> {
    let value = dict?.decode_node(subject)?;
    let label = value.strip_prefix("_:")?;
    Some(label.to_owned())
}

fn subject_label(subject: NodeId, dict: Option<&dyn DictionaryCodec>) -> String {
    if let Some(label) = decode_blank_label(subject, dict)
        && is_safe_ncname(&label)
    {
        return label;
    }
    format!("n{}", subject.get())
}

/// Split a predicate IRI into `(namespace, local)` where `local` is a valid
/// XML NCName. RDF/XML cannot represent predicates that do not split this way.
fn split_ns_local(
    iri: &str,
    _dict: Option<&dyn DictionaryCodec>,
) -> Result<(String, String), OntolithError> {
    let split_at = ['/', '#', ':']
        .into_iter()
        .filter_map(|c| iri.rfind(c))
        .map(|p| p + 1)
        .max()
        .unwrap_or(0);
    let (ns, local) = iri.split_at(split_at);
    if local.is_empty() {
        return Err(OntolithError::failed(format!(
            "rdf/xml: predicate {iri} has no local name"
        )));
    }
    if !is_safe_ncname(local) {
        return Err(OntolithError::failed(format!(
            "rdf/xml: predicate {iri} local name is not an XML NCName"
        )));
    }
    Ok((ns.to_owned(), local.to_owned()))
}

fn namespace_prefix(ns: &str, order: &[String]) -> String {
    if let Some(i) = order.iter().position(|n| n == ns) {
        return format!("ns{i}");
    }
    // rdf namespace is always on the root element.
    format!("ns{}", order.len())
}

fn is_safe_ncname(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn xml_escape_text(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

fn xml_escape_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
}

fn render_subject(node_id: NodeId, dict: Option<&dyn DictionaryCodec>) -> String {
    if let Some(label) = decode_blank_label(node_id, dict)
        && is_safe_blank_label(&label)
    {
        return format!("_:{label}");
    }
    if let Some(dict) = dict
        && let Some(v) = dict.decode_node(node_id)
        && !v.starts_with("_:")
    {
        return render_iri(&Iri::new(&v));
    }
    format!("_:n{}", node_id.get())
}

fn render_iri(iri: &Iri) -> String {
    format!("<{}>", render_iri_text(iri))
}

fn render_iri_text(iri: &Iri) -> String {
    iri.as_str().replace('\\', "\\\\").replace('>', "\\u003E")
}

fn render_term(term: &Term, dict: Option<&dyn DictionaryCodec>) -> String {
    match term {
        Term::Iri(iri) => render_iri(iri),
        Term::BlankNode(id) => {
            if let Some(label) = decode_blank_label(*id, dict)
                && is_safe_blank_label(&label)
            {
                format!("_:{label}")
            } else {
                format!("_:n{}", id.get())
            }
        }
        Term::Literal(literal) => render_literal(literal),
    }
}

fn is_safe_blank_label(label: &str) -> bool {
    let mut chars = label.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c.is_ascii_digit() => {}
        _ => return false,
    }
    let mut last_ok = false;
    for c in chars {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
        if !ok {
            return false;
        }
        last_ok = c != '.';
    }
    last_ok || label.len() == 1
}

fn render_literal(literal: &LiteralValue) -> String {
    match literal {
        LiteralValue::String(value) => format!("\"{}\"", escape_literal(value)),
        LiteralValue::Lang { value, lang } => {
            format!("\"{}\"@{}", escape_literal(value), lang.as_str())
        }
        _ => format!(
            "\"{}\"^^<{}>",
            escape_literal(&literal.lexical_form()),
            literal.xsd_datatype_iri().as_str()
        ),
    }
}

fn escape_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_core::domain::NodeId;
    use ontolith_storage::infrastructure::InMemoryDictionary;

    fn triple(s: u64, p: &str, o: Term) -> Triple {
        Triple::new(NodeId::new(s), Iri::new(p), o)
    }

    fn dict_triple(dict: &InMemoryDictionary, s: &str, p: &str, o: Term) -> Triple {
        let subject = dict.encode_node(s);
        Triple::new(subject, Iri::new(p), o)
    }

    #[test]
    fn ntriples_line_for_iri_object() {
        let t = triple(1, "urn:p", Term::Iri(Iri::new("urn:o")));
        assert_eq!(serialize_triple(&t), "_:n1 <urn:p> <urn:o> .");
    }

    #[test]
    fn ntriples_line_for_literals() {
        let s = triple(
            1,
            "urn:p",
            Term::literal(LiteralValue::String("hi\n\"x\"".into())),
        );
        assert_eq!(serialize_triple(&s), "_:n1 <urn:p> \"hi\\n\\\"x\\\"\" .");

        let i = triple(2, "urn:p", Term::literal(LiteralValue::Integer(-7)));
        assert_eq!(
            serialize_triple(&i),
            "_:n2 <urn:p> \"-7\"^^<http://www.w3.org/2001/XMLSchema#integer> ."
        );

        let b = triple(3, "urn:p", Term::literal(LiteralValue::Boolean(true)));
        assert_eq!(
            serialize_triple(&b),
            "_:n3 <urn:p> \"true\"^^<http://www.w3.org/2001/XMLSchema#boolean> ."
        );
    }

    #[test]
    fn nquads_line_includes_graph() {
        let t = triple(1, "urn:p", Term::Iri(Iri::new("urn:o")));
        let quad = Quad::in_named_graph(t, Iri::new("urn:g"));
        assert_eq!(serialize_quad(&quad), "_:n1 <urn:p> <urn:o> <urn:g> .");
    }

    #[test]
    fn dataset_serializes_by_format() {
        let mut ds = Dataset::new();
        ds.insert_default(triple(1, "urn:p", Term::Iri(Iri::new("urn:o1"))));
        ds.insert_named(
            Iri::new("urn:g"),
            triple(2, "urn:p", Term::Iri(Iri::new("urn:o2"))),
        );

        let nt = serialize_dataset(&ds, SerializeFormat::NTriples);
        assert_eq!(nt, "_:n1 <urn:p> <urn:o1> .\n");

        let nq = serialize_dataset(&ds, SerializeFormat::NQuads);
        assert_eq!(
            nq,
            "_:n1 <urn:p> <urn:o1> .\n_:n2 <urn:p> <urn:o2> <urn:g> .\n"
        );
    }

    #[test]
    fn turtle_and_trig_roundtrip_through_parser() {
        let dict = InMemoryDictionary::new();
        let mut ds = Dataset::new();
        ds.insert_default(dict_triple(
            &dict,
            "urn:alice",
            "urn:knows",
            Term::BlankNode(dict.encode_node("_:b0")),
        ));
        ds.insert_default(dict_triple(
            &dict,
            "urn:alice",
            "urn:name",
            Term::literal(LiteralValue::Lang {
                value: "Alice".into(),
                lang: ontolith_core::domain::LanguageTag::parse("en").unwrap(),
            }),
        ));
        ds.insert_named(
            Iri::new("urn:g1"),
            dict_triple(
                &dict,
                "urn:alice",
                "urn:age",
                Term::literal(LiteralValue::Integer(30)),
            ),
        );

        let turtle = serialize_dataset_with(&ds, SerializeFormat::Turtle, Some(&dict));
        let reparsed = crate::infrastructure::parse_turtle_doc(&turtle, &dict).unwrap();
        let reparsed_default = reparsed.dataset.default_graph.clone();
        assert_eq!(reparsed_default.len(), ds.default_graph.len());
        assert!(
            reparsed_default
                .iter()
                .any(|t| t.predicate.as_str() == "urn:name")
        );

        let trig = serialize_dataset_with(&ds, SerializeFormat::TriG, Some(&dict));
        let re = crate::infrastructure::parse_trig_doc(&trig, &dict).unwrap();
        assert_eq!(re.dataset.named_graphs.len(), 1);
        assert_eq!(re.dataset.named_graphs[0].name.as_str(), "urn:g1");
    }

    #[test]
    fn rdf_xml_roundtrip_and_ncname_error() {
        let dict = InMemoryDictionary::new();
        let mut ds = Dataset::new();
        ds.insert_default(dict_triple(
            &dict,
            "urn:alice",
            "http://example.org/name",
            Term::literal(LiteralValue::Lang {
                value: "Alice".into(),
                lang: ontolith_core::domain::LanguageTag::parse("en").unwrap(),
            }),
        ));
        ds.insert_default(dict_triple(
            &dict,
            "urn:alice",
            "http://example.org/age",
            Term::literal(LiteralValue::Integer(30)),
        ));
        let xml = serialize_rdf_xml(&ds, Some(&dict)).unwrap();
        assert!(xml.contains("rdf:about=\"urn:alice\""));
        assert!(xml.contains("xmlns:ns0=\"http://example.org/\""));
        assert!(xml.contains("<ns0:name xml:lang=\"en\">Alice</ns0:name>"));
        assert!(xml.contains("rdf:datatype=\"http://www.w3.org/2001/XMLSchema#integer\""));

        // Round-trip: parse the generated document back into the dataset.
        let reparsed = crate::infrastructure::parse_rdf_xml_doc(&xml, &dict, None).unwrap();
        assert_eq!(reparsed.dataset.triple_count(), 2);

        // Predicate without a local name is not expressible.
        let mut bad = Dataset::new();
        bad.insert_default(dict_triple(
            &dict,
            "urn:alice",
            "urn:",
            Term::literal(LiteralValue::String("x".into())),
        ));
        assert!(serialize_rdf_xml(&bad, Some(&dict)).is_err());
    }

    #[test]
    fn format_name_is_stable() {
        assert_eq!(SerializeFormat::NTriples.as_str(), "n-triples");
        assert_eq!(SerializeFormat::NQuads.as_str(), "n-quads");
        assert_eq!(SerializeFormat::Turtle.as_str(), "turtle");
        assert_eq!(SerializeFormat::TriG.as_str(), "trig");
    }
}
