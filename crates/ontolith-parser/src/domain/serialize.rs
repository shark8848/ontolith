//! RDF syntax serialization (N-Triples / N-Quads writers).
//!
//! Counterpart of the parse surface: deterministic, line-oriented output
//! suitable for export and interop. Literal lexical forms reuse the L0
//! `LiteralValue` canonical lexicalization so writer output round-trips
//! through the reader.

use ontolith_core::domain::{Iri, LiteralValue};
use ontolith_rdf::domain::{Dataset, Quad, Term, Triple};

/// Supported RDF serialization formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerializeFormat {
    NTriples,
    NQuads,
}

impl SerializeFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NTriples => "n-triples",
            Self::NQuads => "n-quads",
        }
    }
}

/// Render a single triple as an N-Triples line (no trailing newline).
pub fn serialize_triple(triple: &Triple) -> String {
    format!(
        "{} {} {} .",
        render_subject(triple.subject.get()),
        render_iri(&triple.predicate),
        render_term(&triple.object)
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

/// Serialize a dataset; N-Triples emits default-graph triples, N-Quads emits
/// every quad (default and named graphs).
pub fn serialize_dataset(dataset: &Dataset, format: SerializeFormat) -> String {
    let mut out = String::new();
    for quad in dataset.quads() {
        let line = match format {
            SerializeFormat::NTriples if quad.graph_name.is_some() => continue,
            SerializeFormat::NTriples => serialize_triple(&quad.triple),
            SerializeFormat::NQuads => serialize_quad(&quad),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn render_subject(node_id: u64) -> String {
    format!("_:n{node_id}")
}

fn render_iri(iri: &Iri) -> String {
    format!("<{}>", render_iri_text(iri))
}

fn render_iri_text(iri: &Iri) -> String {
    iri.as_str().replace('\\', "\\\\").replace('>', "\\u003E")
}

fn render_term(term: &Term) -> String {
    match term {
        Term::Iri(iri) => render_iri(iri),
        Term::BlankNode(id) => render_subject(id.get()),
        Term::Literal(literal) => render_literal(literal),
    }
}

fn render_literal(literal: &LiteralValue) -> String {
    match literal {
        // Plain string literals use the simple (datatype-free) N-Triples form.
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
    use ontolith_core::domain::{Iri, NodeId};

    fn triple(s: u64, p: &str, o: Term) -> Triple {
        Triple::new(NodeId::new(s), Iri::new(p), o)
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
    fn format_name_is_stable() {
        assert_eq!(SerializeFormat::NTriples.as_str(), "n-triples");
        assert_eq!(SerializeFormat::NQuads.as_str(), "n-quads");
    }
}
