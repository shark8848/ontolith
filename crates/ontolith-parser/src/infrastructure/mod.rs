//! Parser infrastructure adapters (L3 — full syntax surface).

mod nt;
mod term_lex;
mod turtle;

use crate::application::RdfParser;
use crate::domain::{DatasetSink, ParseFormat, ParseOutput, ParseRequest, RdfEventSink};
use ontolith_core::error::OntolithError;
use ontolith_storage::application::DictionaryCodec;

use self::nt::{LineFormat, parse_document_streaming};
use self::turtle::{parse_trig, parse_turtle};

/// Production RDF parser: N-Triples, N-Quads, Turtle, TriG.
#[derive(Debug, Default, Clone, Copy)]
pub struct BasicRdfParser;

impl BasicRdfParser {
    pub fn new() -> Self {
        Self
    }
}

impl RdfParser for BasicRdfParser {
    fn parse(
        &self,
        request: &ParseRequest,
        input: &str,
        dictionary: &dyn DictionaryCodec,
    ) -> Result<ParseOutput, OntolithError> {
        let mut sink = DatasetSink::default();
        self.parse_streaming(request, input, dictionary, &mut sink)?;
        // Normalize stats for N-Triples (quad_count stays 0).
        let mut stats = sink.stats;
        stats.line_count = input.lines().count();
        if matches!(request.format, ParseFormat::NTriples) {
            stats.quad_count = 0;
            stats.triple_count = sink.dataset.triple_count();
        } else if matches!(request.format, ParseFormat::NQuads) {
            stats.triple_count = sink.dataset.default_graph.len();
            stats.quad_count = sink.dataset.triple_count();
        } else {
            // Turtle / TriG: triple_count from sink events; add named graph triples into totals.
            stats.triple_count = sink.dataset.default_graph.len();
            stats.quad_count = sink
                .dataset
                .named_graphs
                .iter()
                .map(|g| g.triples.len())
                .sum();
        }
        Ok(ParseOutput {
            dataset: sink.dataset,
            stats,
        })
    }

    fn parse_streaming(
        &self,
        request: &ParseRequest,
        input: &str,
        dictionary: &dyn DictionaryCodec,
        sink: &mut dyn RdfEventSink,
    ) -> Result<(), OntolithError> {
        match request.format {
            ParseFormat::NTriples => {
                parse_document_streaming(input, LineFormat::NTriples, dictionary, sink)
            }
            ParseFormat::NQuads => {
                parse_document_streaming(input, LineFormat::NQuads, dictionary, sink)
            }
            ParseFormat::Turtle => parse_turtle(input, dictionary, request.base_iri.clone(), sink),
            ParseFormat::TriG => parse_trig(input, dictionary, request.base_iri.clone(), sink),
            ParseFormat::JsonLd => Err(OntolithError::Unsupported("json-ld")),
        }
    }
}

pub fn parse_ntriples(
    input: &str,
    dictionary: &dyn DictionaryCodec,
) -> Result<ParseOutput, OntolithError> {
    BasicRdfParser::new().parse(&ParseRequest::ntriples("inline"), input, dictionary)
}

pub fn parse_nquads(
    input: &str,
    dictionary: &dyn DictionaryCodec,
) -> Result<ParseOutput, OntolithError> {
    BasicRdfParser::new().parse(&ParseRequest::nquads("inline"), input, dictionary)
}

pub fn parse_turtle_doc(
    input: &str,
    dictionary: &dyn DictionaryCodec,
) -> Result<ParseOutput, OntolithError> {
    BasicRdfParser::new().parse(&ParseRequest::turtle("inline"), input, dictionary)
}

pub fn parse_trig_doc(
    input: &str,
    dictionary: &dyn DictionaryCodec,
) -> Result<ParseOutput, OntolithError> {
    BasicRdfParser::new().parse(&ParseRequest::trig("inline"), input, dictionary)
}

pub fn status() -> &'static str {
    "infrastructure"
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_core::domain::{Iri, LiteralValue};
    use ontolith_rdf::domain::Term;
    use ontolith_storage::infrastructure::InMemoryDictionary;

    #[test]
    fn parses_ntriples_iris_and_literals() {
        let dict = InMemoryDictionary::new();
        let input = r#"
# comment
<http://ex.org/alice> <http://ex.org/knows> <http://ex.org/bob> .
<http://ex.org/alice> <http://ex.org/name> "Alice" .
<http://ex.org/alice> <http://ex.org/age> "42"^^<http://www.w3.org/2001/XMLSchema#integer> .
"#;
        let out = parse_ntriples(input, &dict).expect("parse");
        assert_eq!(out.stats.triple_count, 3);
        let alice = dict.encode_node("http://ex.org/alice");
        assert!(
            out.dataset
                .default_graph
                .iter()
                .any(|t| { t.subject == alice && t.predicate.as_str() == "http://ex.org/name" })
        );
        assert!(
            out.dataset
                .default_graph
                .iter()
                .any(|t| { matches!(&t.object, Term::Literal(LiteralValue::Integer(42))) })
        );
    }

    #[test]
    fn parses_blank_nodes_consistently() {
        let dict = InMemoryDictionary::new();
        let input = r#"
_:b1 <http://ex.org/p> <http://ex.org/o> .
_:b1 <http://ex.org/p2> "x" .
"#;
        let out = parse_ntriples(input, &dict).expect("parse");
        assert_eq!(
            out.dataset.default_graph[0].subject,
            out.dataset.default_graph[1].subject
        );
    }

    #[test]
    fn parses_nquads_named_graph() {
        let dict = InMemoryDictionary::new();
        let input = r#"
<http://ex.org/s> <http://ex.org/p> <http://ex.org/o> <http://ex.org/g> .
<http://ex.org/s2> <http://ex.org/p> "lit" .
"#;
        let out = parse_nquads(input, &dict).expect("parse");
        assert_eq!(out.dataset.graph_count(), 2);
        assert_eq!(
            out.dataset
                .named_graph(&Iri::new("http://ex.org/g"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn parses_turtle_prefix_and_predicate_lists() {
        let dict = InMemoryDictionary::new();
        let input = r#"
@prefix ex: <http://ex.org/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .

ex:alice a foaf:Person ;
    foaf:name "Alice" ;
    foaf:knows ex:bob, ex:carol .
"#;
        let out = parse_turtle_doc(input, &dict).expect("turtle");
        assert!(out.dataset.triple_count() >= 4);
        let alice = dict.encode_node("http://ex.org/alice");
        let type_p = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        assert!(
            out.dataset
                .default_graph
                .iter()
                .any(|t| { t.subject == alice && t.predicate.as_str() == type_p })
        );
        assert_eq!(
            out.dataset
                .default_graph
                .iter()
                .filter(|t| t.subject == alice && t.predicate.as_str().ends_with("knows"))
                .count(),
            2
        );
    }

    #[test]
    fn parses_turtle_blank_node_property_list() {
        let dict = InMemoryDictionary::new();
        let input = r#"
@prefix ex: <http://ex.org/> .
ex:alice ex:address [ ex:city "NYC" ; ex:zip "10001" ] .
"#;
        let out = parse_turtle_doc(input, &dict).expect("turtle");
        assert!(out.dataset.triple_count() >= 3);
    }

    #[test]
    fn parses_turtle_collection() {
        let dict = InMemoryDictionary::new();
        let input = r#"
@prefix ex: <http://ex.org/> .
ex:list ex:items ( ex:a ex:b ex:c ) .
"#;
        let out = parse_turtle_doc(input, &dict).expect("turtle");
        // list link + 3 first + 3 rest (last to nil) = 1 + 3 + 3 = 7
        assert!(out.dataset.triple_count() >= 7);
    }

    #[test]
    fn parses_trig_named_graph() {
        let dict = InMemoryDictionary::new();
        let input = r#"
@prefix ex: <http://ex.org/> .
ex:g {
  ex:s ex:p ex:o .
}
{ ex:s2 ex:p "default" . }
"#;
        let out = parse_trig_doc(input, &dict).expect("trig");
        assert_eq!(out.dataset.default_graph.len(), 1);
        assert_eq!(
            out.dataset
                .named_graph(&Iri::new("http://ex.org/g"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn streaming_emits_events() {
        let dict = InMemoryDictionary::new();
        let mut sink = DatasetSink::default();
        let parser = BasicRdfParser::new();
        parser
            .parse_streaming(
                &ParseRequest::turtle("x"),
                r#"@prefix ex: <http://ex.org/> . ex:s ex:p ex:o ."#,
                &dict,
                &mut sink,
            )
            .unwrap();
        assert_eq!(sink.dataset.triple_count(), 1);
        assert!(sink.stats.prefix_count >= 1);
    }

    #[test]
    fn rejects_bad_ntriples_line() {
        let dict = InMemoryDictionary::new();
        let err =
            parse_ntriples("<http://ex.org/s> <http://ex.org/p> .", &dict).expect_err("must fail");
        assert!(matches!(
            err,
            OntolithError::InvalidArgument(_) | OntolithError::Failed(_)
        ));
    }

    #[test]
    fn unsupported_jsonld() {
        let dict = InMemoryDictionary::new();
        let err = BasicRdfParser::new()
            .parse(&ParseRequest::new(ParseFormat::JsonLd, "x"), "{}", &dict)
            .unwrap_err();
        assert_eq!(err, OntolithError::Unsupported("json-ld"));
    }

    #[test]
    fn turtle_error_includes_location() {
        let dict = InMemoryDictionary::new();
        let err = parse_turtle_doc("@prefix ex: <http://ex.org/> . ex:s .", &dict).unwrap_err();
        let msg = err.message();
        assert!(
            msg.contains("parse error") || msg.contains("expected"),
            "{msg}"
        );
    }
}
