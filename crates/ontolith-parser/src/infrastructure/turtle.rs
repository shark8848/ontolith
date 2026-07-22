//! Turtle and TriG parsers (full L3 syntax surface for RDF exchange).
//!
//! Supported:
//! - `@prefix` / `PREFIX`, `@base` / `BASE`
//! - Prefixed names, `a` keyword, absolute IRIs
//! - Predicate lists (`;`) and object lists (`,`)
//! - Quoted / long strings, language tags, datatypes
//! - Blank nodes `_:x` and `[]` / `[ p o ; ... ]`
//! - Collections `( ... )` expanded to rdf:first/rest/nil
//! - TriG named graphs: `<g> { ... }` / `GRAPH <g> { ... }` / bare `{ ... }`
//!
//! Not supported: RDF-star annotation syntax.

use super::term_lex::{
    Lexer, PrefixMap, Tok, TurtleTerm, object_term, predicate_iri, subject_node,
};
use crate::domain::{RdfEvent, RdfEventSink};
use ontolith_core::domain::Iri;
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Quad, Triple};
use ontolith_storage::application::DictionaryCodec;

const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

pub struct TurtleParser<'a> {
    lex: Lexer<'a>,
    prefixes: PrefixMap,
    dictionary: &'a dyn DictionaryCodec,
    current_graph: Option<Iri>,
    allow_graphs: bool,
    lookahead: Option<Tok>,
}

impl<'a> TurtleParser<'a> {
    pub fn new(
        input: &'a str,
        dictionary: &'a dyn DictionaryCodec,
        base: Option<String>,
        allow_graphs: bool,
    ) -> Self {
        Self {
            lex: Lexer::new(input),
            prefixes: PrefixMap::with_base(base),
            dictionary,
            current_graph: None,
            allow_graphs,
            lookahead: None,
        }
    }

    pub fn parse_into(&mut self, sink: &mut dyn RdfEventSink) -> Result<(), OntolithError> {
        loop {
            let tok = self.peek()?;
            if tok == Tok::Eof {
                break;
            }
            match tok {
                Tok::AtPrefix | Tok::Prefix => self.parse_prefix_decl(sink)?,
                Tok::AtBase | Tok::Base => self.parse_base_decl(sink)?,
                Tok::OpenBrace if self.allow_graphs => {
                    self.bump()?;
                    self.parse_graph_body(sink, None)?;
                }
                _ if self.allow_graphs => {
                    // GRAPH? iri { ... }  OR  iri { ... }  OR  triples
                    if self.is_graph_start()? {
                        self.parse_named_graph(sink)?;
                    } else {
                        self.parse_triples(sink)?;
                    }
                }
                _ => self.parse_triples(sink)?,
            }
        }
        Ok(())
    }

    fn is_graph_start(&mut self) -> Result<bool, OntolithError> {
        let tok = self.peek()?;
        if matches!(&tok, Tok::Term(TurtleTerm::Bare(s)) if s.eq_ignore_ascii_case("graph")) {
            return Ok(true);
        }
        // iri `{`
        if matches!(
            tok,
            Tok::Term(TurtleTerm::IriRef(_))
                | Tok::Term(TurtleTerm::Prefixed(_))
                | Tok::Term(TurtleTerm::Bare(_))
        ) {
            // Look ahead without consuming more than we can restore: parse is single-pass;
            // try reading next token after cloning position is hard — use temporary scan.
            let saved_pos = self.lex.pos;
            let saved_line = self.lex.line;
            let saved_col = self.lex.col;
            let saved_la = self.lookahead.clone();
            let _ = self.bump()?; // iri
            let next = self.peek()?;
            let is_graph = next == Tok::OpenBrace;
            // restore
            self.lex.pos = saved_pos;
            self.lex.line = saved_line;
            self.lex.col = saved_col;
            self.lookahead = saved_la;
            return Ok(is_graph);
        }
        Ok(false)
    }

    fn parse_named_graph(&mut self, sink: &mut dyn RdfEventSink) -> Result<(), OntolithError> {
        let mut tok = self.bump()?;
        if matches!(&tok, Tok::Term(TurtleTerm::Bare(s)) if s.eq_ignore_ascii_case("graph")) {
            tok = self.bump()?;
        }
        let graph = self.term_to_graph_iri(&tok)?;
        self.expect(Tok::OpenBrace)?;
        self.parse_graph_body(sink, Some(graph))?;
        Ok(())
    }

    fn parse_graph_body(
        &mut self,
        sink: &mut dyn RdfEventSink,
        graph: Option<Iri>,
    ) -> Result<(), OntolithError> {
        let prev = self.current_graph.clone();
        self.current_graph = graph;
        while self.peek()? != Tok::CloseBrace && self.peek()? != Tok::Eof {
            self.parse_triples(sink)?;
        }
        self.expect(Tok::CloseBrace)?;
        self.current_graph = prev;
        Ok(())
    }

    fn parse_prefix_decl(&mut self, sink: &mut dyn RdfEventSink) -> Result<(), OntolithError> {
        self.bump()?; // PREFIX / @prefix
        let name_tok = self.bump()?;
        let prefix = match name_tok {
            Tok::Term(TurtleTerm::Prefixed(s)) => s.trim_end_matches(':').to_owned(),
            Tok::Term(TurtleTerm::Bare(s)) => s.trim_end_matches(':').to_owned(),
            Tok::Dot => String::new(), // rare `: <ns>`
            other => {
                return Err(OntolithError::parse_at(
                    self.lex.line,
                    self.lex.col,
                    format!("expected prefix name, got {other:?}"),
                ));
            }
        };
        // If bare was `foo:` it already handled; if we got bare `foo` expect ':'
        // Prefixed form "foo:" comes as Prefixed("foo:") from lexer when colon present.
        let iri_tok = self.bump()?;
        let iri = match iri_tok {
            Tok::Term(TurtleTerm::IriRef(s)) => s,
            _ => {
                return Err(OntolithError::parse_at(
                    self.lex.line,
                    self.lex.col,
                    "prefix value must be an IRI",
                ));
            }
        };
        self.prefixes.set_prefix(prefix.clone(), iri.clone());
        sink.on_event(RdfEvent::Prefix { prefix, iri })?;
        if self.peek()? == Tok::Dot {
            self.bump()?;
        }
        Ok(())
    }

    fn parse_base_decl(&mut self, sink: &mut dyn RdfEventSink) -> Result<(), OntolithError> {
        self.bump()?;
        let iri_tok = self.bump()?;
        let iri = match iri_tok {
            Tok::Term(TurtleTerm::IriRef(s)) => s,
            _ => {
                return Err(OntolithError::parse_at(
                    self.lex.line,
                    self.lex.col,
                    "base value must be an IRI",
                ));
            }
        };
        self.prefixes.set_base(iri.clone());
        sink.on_event(RdfEvent::Base(iri))?;
        if self.peek()? == Tok::Dot {
            self.bump()?;
        }
        Ok(())
    }

    fn parse_triples(&mut self, sink: &mut dyn RdfEventSink) -> Result<(), OntolithError> {
        if self.peek()? == Tok::Dot {
            self.bump()?;
            return Ok(());
        }
        let subject = self.parse_node(sink)?;
        self.parse_predicate_object_list(sink, &subject)?;
        if self.peek()? == Tok::Dot {
            self.bump()?;
        }
        Ok(())
    }

    fn parse_predicate_object_list(
        &mut self,
        sink: &mut dyn RdfEventSink,
        subject: &SubjectRef,
    ) -> Result<(), OntolithError> {
        loop {
            let pred = self.parse_predicate()?;
            self.parse_object_list(sink, subject, &pred)?;
            if self.peek()? == Tok::Semicolon {
                self.bump()?;
                // allow trailing semicolon
                let next = self.peek()?;
                if matches!(
                    next,
                    Tok::Dot | Tok::CloseBrace | Tok::CloseBracket | Tok::Eof
                ) {
                    break;
                }
                continue;
            }
            break;
        }
        Ok(())
    }

    fn parse_object_list(
        &mut self,
        sink: &mut dyn RdfEventSink,
        subject: &SubjectRef,
        predicate: &Iri,
    ) -> Result<(), OntolithError> {
        loop {
            let object = self.parse_object(sink)?;
            self.emit_triple(sink, subject, predicate, object)?;
            if self.peek()? == Tok::Comma {
                self.bump()?;
                continue;
            }
            break;
        }
        Ok(())
    }

    fn parse_predicate(&mut self) -> Result<Iri, OntolithError> {
        let line = self.lex.line;
        let col = self.lex.col;
        let tok = self.bump()?;
        match tok {
            Tok::A => Ok(Iri::new(RDF_TYPE)),
            Tok::Term(t) => predicate_iri(&self.prefixes, &t, line, col),
            other => Err(OntolithError::parse_at(
                line,
                col,
                format!("expected predicate, got {other:?}"),
            )),
        }
    }

    fn parse_node(&mut self, sink: &mut dyn RdfEventSink) -> Result<SubjectRef, OntolithError> {
        let line = self.lex.line;
        let col = self.lex.col;
        let tok = self.bump()?;
        match tok {
            Tok::Term(t) => {
                let id = subject_node(self.dictionary, &self.prefixes, &t, line, col)?;
                Ok(SubjectRef::Node(id))
            }
            Tok::OpenBracket => self.parse_blank_node_property_list(sink),
            Tok::OpenParen => self.parse_collection(sink).map(SubjectRef::Node),
            other => Err(OntolithError::parse_at(
                line,
                col,
                format!("expected subject, got {other:?}"),
            )),
        }
    }

    fn parse_object(
        &mut self,
        sink: &mut dyn RdfEventSink,
    ) -> Result<ontolith_rdf::domain::Term, OntolithError> {
        let line = self.lex.line;
        let col = self.lex.col;
        let tok = self.peek()?;
        match tok {
            Tok::OpenBracket => {
                self.bump()?;
                let subj = self.parse_blank_node_property_list(sink)?;
                Ok(ontolith_rdf::domain::Term::BlankNode(subj.node_id()))
            }
            Tok::OpenParen => {
                self.bump()?;
                let id = self.parse_collection(sink)?;
                Ok(ontolith_rdf::domain::Term::BlankNode(id))
            }
            _ => {
                let tok = self.bump()?;
                match tok {
                    Tok::Term(t) => object_term(self.dictionary, &self.prefixes, &t, line, col),
                    other => Err(OntolithError::parse_at(
                        line,
                        col,
                        format!("expected object, got {other:?}"),
                    )),
                }
            }
        }
    }

    fn parse_blank_node_property_list(
        &mut self,
        sink: &mut dyn RdfEventSink,
    ) -> Result<SubjectRef, OntolithError> {
        // caller already consumed '['
        let label = self.prefixes.mint_blank();
        let id = self.dictionary.encode_node(&format!("_:{label}"));
        let subject = SubjectRef::Node(id);
        if self.peek()? != Tok::CloseBracket {
            self.parse_predicate_object_list(sink, &subject)?;
        }
        self.expect(Tok::CloseBracket)?;
        Ok(subject)
    }

    fn parse_collection(
        &mut self,
        sink: &mut dyn RdfEventSink,
    ) -> Result<ontolith_core::domain::NodeId, OntolithError> {
        // caller consumed '('
        let mut items = Vec::new();
        while self.peek()? != Tok::CloseParen && self.peek()? != Tok::Eof {
            items.push(self.parse_object(sink)?);
        }
        self.expect(Tok::CloseParen)?;
        if items.is_empty() {
            return Ok(self.dictionary.encode_node(RDF_NIL));
        }
        let mut head = None;
        let mut prev: Option<ontolith_core::domain::NodeId> = None;
        for item in items {
            let node_label = self.prefixes.mint_blank();
            let node = self.dictionary.encode_node(&format!("_:{node_label}"));
            if head.is_none() {
                head = Some(node);
            }
            if let Some(p) = prev {
                self.emit_triple(
                    sink,
                    &SubjectRef::Node(p),
                    &Iri::new(RDF_REST),
                    ontolith_rdf::domain::Term::BlankNode(node),
                )?;
            }
            self.emit_triple(sink, &SubjectRef::Node(node), &Iri::new(RDF_FIRST), item)?;
            prev = Some(node);
        }
        if let Some(p) = prev {
            self.emit_triple(
                sink,
                &SubjectRef::Node(p),
                &Iri::new(RDF_REST),
                ontolith_rdf::domain::Term::Iri(Iri::new(RDF_NIL)),
            )?;
        }
        Ok(head.unwrap())
    }

    fn emit_triple(
        &mut self,
        sink: &mut dyn RdfEventSink,
        subject: &SubjectRef,
        predicate: &Iri,
        object: ontolith_rdf::domain::Term,
    ) -> Result<(), OntolithError> {
        let triple = Triple::new(subject.node_id(), predicate.clone(), object);
        if let Some(g) = &self.current_graph {
            sink.on_event(RdfEvent::Quad(Quad::in_named_graph(triple, g.clone())))?;
        } else {
            sink.on_event(RdfEvent::Triple(triple))?;
        }
        Ok(())
    }

    fn term_to_graph_iri(&self, tok: &Tok) -> Result<Iri, OntolithError> {
        let line = self.lex.line;
        let col = self.lex.col;
        match tok {
            Tok::Term(t) => predicate_iri(&self.prefixes, t, line, col),
            _ => Err(OntolithError::parse_at(line, col, "graph name must be IRI")),
        }
    }

    fn peek(&mut self) -> Result<Tok, OntolithError> {
        if self.lookahead.is_none() {
            self.lookahead = Some(self.lex.next_token()?);
        }
        Ok(self.lookahead.clone().unwrap())
    }

    fn bump(&mut self) -> Result<Tok, OntolithError> {
        if let Some(t) = self.lookahead.take() {
            return Ok(t);
        }
        self.lex.next_token()
    }

    fn expect(&mut self, expected: Tok) -> Result<(), OntolithError> {
        let got = self.bump()?;
        if got != expected {
            return Err(OntolithError::parse_at(
                self.lex.line,
                self.lex.col,
                format!("expected {expected:?}, got {got:?}"),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
enum SubjectRef {
    Node(ontolith_core::domain::NodeId),
}

impl SubjectRef {
    fn node_id(&self) -> ontolith_core::domain::NodeId {
        match self {
            Self::Node(id) => *id,
        }
    }
}

pub fn parse_turtle(
    input: &str,
    dictionary: &dyn DictionaryCodec,
    base: Option<String>,
    sink: &mut dyn RdfEventSink,
) -> Result<(), OntolithError> {
    let mut parser = TurtleParser::new(input, dictionary, base, false);
    parser.parse_into(sink)
}

pub fn parse_trig(
    input: &str,
    dictionary: &dyn DictionaryCodec,
    base: Option<String>,
    sink: &mut dyn RdfEventSink,
) -> Result<(), OntolithError> {
    let mut parser = TurtleParser::new(input, dictionary, base, true);
    parser.parse_into(sink)
}
