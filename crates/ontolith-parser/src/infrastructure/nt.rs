//! N-Triples / N-Quads line-oriented parsers (L3).
//!
//! Supported term forms:
//! - IRIs: `<http://example.org/x>`
//! - Blank nodes: `_:label`
//! - Simple / language / datatype literals:
//!   - `"text"`
//!   - `"text"@en`
//!   - `"text"^^<http://www.w3.org/2001/XMLSchema#string>`
//!
//! Streaming via [`parse_document_streaming`]. RDF-star is not supported.

use crate::domain::{RdfEvent, RdfEventSink};
use ontolith_core::domain::{Iri, LiteralValue, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Quad, Term, Triple};
use ontolith_storage::application::DictionaryCodec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineFormat {
    NTriples,
    NQuads,
}

/// Stream N-Triples / N-Quads statements into `sink`.
pub fn parse_document_streaming(
    input: &str,
    format: LineFormat,
    dictionary: &dyn DictionaryCodec,
    sink: &mut dyn RdfEventSink,
) -> Result<(), OntolithError> {
    let mut line_no = 0usize;
    for raw_line in input.lines() {
        line_no += 1;
        let line = strip_line_comment(raw_line);
        let line = line.trim();
        if line.is_empty() {
            if raw_line.trim().starts_with('#') {
                sink.on_event(RdfEvent::Comment)?;
            }
            continue;
        }
        if raw_line.trim_start().starts_with('#') {
            sink.on_event(RdfEvent::Comment)?;
            continue;
        }

        match format {
            LineFormat::NTriples => {
                let triple = parse_ntriples_line(line, line_no, dictionary)?;
                sink.on_event(RdfEvent::Triple(triple))?;
            }
            LineFormat::NQuads => {
                let quad = parse_nquads_line(line, line_no, dictionary)?;
                sink.on_event(RdfEvent::Quad(quad))?;
            }
        }
    }
    Ok(())
}

fn strip_line_comment(line: &str) -> &str {
    // Only treat `#` as comment when it starts the line (after whitespace).
    // Inline `#` inside IRIs/literals is preserved by the tokenizer.
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') { "" } else { line }
}

fn parse_ntriples_line(
    line: &str,
    line_no: usize,
    dictionary: &dyn DictionaryCodec,
) -> Result<Triple, OntolithError> {
    let tokens = tokenize_statement(line, line_no)?;
    if tokens.len() != 3 {
        return Err(line_err(
            line_no,
            "n-triples statement must have exactly 3 terms before '.'",
        ));
    }
    let subject = parse_subject(&tokens[0], line_no, dictionary)?;
    let predicate = parse_predicate(&tokens[1], line_no)?;
    let object = parse_object(&tokens[2], line_no, dictionary)?;
    Ok(Triple::new(subject, predicate, object))
}

fn parse_nquads_line(
    line: &str,
    line_no: usize,
    dictionary: &dyn DictionaryCodec,
) -> Result<Quad, OntolithError> {
    let tokens = tokenize_statement(line, line_no)?;
    match tokens.len() {
        3 => {
            let triple = Triple::new(
                parse_subject(&tokens[0], line_no, dictionary)?,
                parse_predicate(&tokens[1], line_no)?,
                parse_object(&tokens[2], line_no, dictionary)?,
            );
            Ok(Quad::in_default_graph(triple))
        }
        4 => {
            let triple = Triple::new(
                parse_subject(&tokens[0], line_no, dictionary)?,
                parse_predicate(&tokens[1], line_no)?,
                parse_object(&tokens[2], line_no, dictionary)?,
            );
            let graph = parse_graph_name(&tokens[3], line_no)?;
            Ok(Quad::in_named_graph(triple, graph))
        }
        _ => Err(line_err(
            line_no,
            "n-quads statement must have 3 or 4 terms before '.'",
        )),
    }
}

fn tokenize_statement(line: &str, line_no: usize) -> Result<Vec<String>, OntolithError> {
    let mut tokens = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'.' {
            // trailing dot
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i != bytes.len() {
                return Err(line_err(line_no, "unexpected tokens after statement '.'"));
            }
            break;
        }

        let (token, next) = match bytes[i] {
            b'<' => take_iri(line, i, line_no)?,
            b'"' => take_literal(line, i, line_no)?,
            b'_' => take_blank(line, i, line_no)?,
            _ => {
                return Err(line_err(
                    line_no,
                    "unexpected token; expected IRI, blank node, or literal",
                ));
            }
        };
        tokens.push(token);
        i = next;
    }

    if tokens.is_empty() {
        return Err(line_err(line_no, "empty statement"));
    }
    Ok(tokens)
}

fn take_iri(line: &str, start: usize, line_no: usize) -> Result<(String, usize), OntolithError> {
    let bytes = line.as_bytes();
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'>' {
            let iri = &line[start..=i];
            return Ok((iri.to_owned(), i + 1));
        }
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            return Err(line_err(line_no, "whitespace inside IRI is not allowed"));
        }
        i += 1;
    }
    Err(line_err(line_no, "unterminated IRI"))
}

fn take_blank(line: &str, start: usize, line_no: usize) -> Result<(String, usize), OntolithError> {
    let bytes = line.as_bytes();
    if start + 1 >= bytes.len() || bytes[start + 1] != b':' {
        return Err(line_err(line_no, "blank node must start with '_:'"));
    }
    let mut i = start + 2;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() || b == b'.' {
            break;
        }
        if !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.') {
            return Err(line_err(line_no, "invalid blank node label character"));
        }
        i += 1;
    }
    if i == start + 2 {
        return Err(line_err(line_no, "blank node label must not be empty"));
    }
    Ok((line[start..i].to_owned(), i))
}

fn take_literal(
    line: &str,
    start: usize,
    line_no: usize,
) -> Result<(String, usize), OntolithError> {
    let bytes = line.as_bytes();
    let mut i = start + 1;
    let mut escaped = false;
    while i < bytes.len() {
        let b = bytes[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if b == b'\\' {
            escaped = true;
            i += 1;
            continue;
        }
        if b == b'"' {
            i += 1;
            // optional @lang or ^^<datatype>
            if i < bytes.len() && bytes[i] == b'@' {
                i += 1;
                let lang_start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
                    i += 1;
                }
                if i == lang_start {
                    return Err(line_err(line_no, "empty language tag"));
                }
            } else if i + 1 < bytes.len() && bytes[i] == b'^' && bytes[i + 1] == b'^' {
                i += 2;
                if i >= bytes.len() || bytes[i] != b'<' {
                    return Err(line_err(line_no, "datatype must be an IRI"));
                }
                let (_dt, next) = take_iri(line, i, line_no)?;
                i = next;
            }
            return Ok((line[start..i].to_owned(), i));
        }
        i += 1;
    }
    Err(line_err(line_no, "unterminated literal"))
}

fn parse_subject(
    token: &str,
    line_no: usize,
    dictionary: &dyn DictionaryCodec,
) -> Result<NodeId, OntolithError> {
    if let Some(iri) = as_iri(token) {
        return Ok(dictionary.encode_node(iri));
    }
    if let Some(label) = as_blank(token) {
        // Blank nodes are dictionary-encoded under a reserved lexical form.
        return Ok(dictionary.encode_node(&format!("_:{}", label)));
    }
    Err(line_err(line_no, "subject must be an IRI or blank node"))
}

fn parse_predicate(token: &str, line_no: usize) -> Result<Iri, OntolithError> {
    let Some(iri) = as_iri(token) else {
        return Err(line_err(line_no, "predicate must be an IRI"));
    };
    Iri::parse(iri).map_err(|_| line_err(line_no, "invalid predicate IRI"))
}

fn parse_object(
    token: &str,
    line_no: usize,
    dictionary: &dyn DictionaryCodec,
) -> Result<Term, OntolithError> {
    if let Some(iri) = as_iri(token) {
        return Ok(Term::Iri(
            Iri::parse(iri).map_err(|_| line_err(line_no, "invalid object IRI"))?,
        ));
    }
    if let Some(label) = as_blank(token) {
        let id = dictionary.encode_node(&format!("_:{}", label));
        return Ok(Term::BlankNode(id));
    }
    if token.starts_with('"') {
        return Ok(Term::Literal(parse_literal_value(token, line_no)?));
    }
    Err(line_err(
        line_no,
        "object must be an IRI, blank node, or literal",
    ))
}

fn parse_graph_name(token: &str, line_no: usize) -> Result<Iri, OntolithError> {
    let Some(iri) = as_iri(token) else {
        return Err(line_err(
            line_no,
            "graph name must be an IRI in baseline parser",
        ));
    };
    Iri::parse(iri).map_err(|_| line_err(line_no, "invalid graph name IRI"))
}

fn parse_literal_value(token: &str, line_no: usize) -> Result<LiteralValue, OntolithError> {
    // token starts with '"'
    let bytes = token.as_bytes();
    let mut i = 1usize;
    let mut escaped = false;
    let mut content = String::new();
    while i < bytes.len() {
        let b = bytes[i];
        if escaped {
            content.push(match b {
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                b'"' => '"',
                b'\\' => '\\',
                other => other as char,
            });
            escaped = false;
            i += 1;
            continue;
        }
        if b == b'\\' {
            escaped = true;
            i += 1;
            continue;
        }
        if b == b'"' {
            i += 1;
            break;
        }
        content.push(b as char);
        i += 1;
    }
    if i == 0 {
        return Err(line_err(line_no, "invalid literal"));
    }

    // language / datatype suffix ignored for compact LiteralValue payload except
    // when datatype is a known XSD numeric/boolean.
    if i < bytes.len() && bytes[i] == b'@' {
        return Ok(LiteralValue::String(content));
    }
    if i + 1 < bytes.len() && bytes[i] == b'^' && bytes[i + 1] == b'^' {
        let dt = &token[i + 2..];
        if let Some(iri) = as_iri(dt) {
            return Ok(coerce_typed_literal(content, iri));
        }
        return Err(line_err(line_no, "invalid datatype IRI on literal"));
    }
    Ok(LiteralValue::String(content))
}

fn coerce_typed_literal(content: String, datatype: &str) -> LiteralValue {
    match datatype {
        "http://www.w3.org/2001/XMLSchema#integer"
        | "http://www.w3.org/2001/XMLSchema#int"
        | "http://www.w3.org/2001/XMLSchema#long" => content
            .parse::<i64>()
            .map(LiteralValue::Integer)
            .unwrap_or(LiteralValue::String(content)),
        "http://www.w3.org/2001/XMLSchema#double"
        | "http://www.w3.org/2001/XMLSchema#float"
        | "http://www.w3.org/2001/XMLSchema#decimal" => content
            .parse::<f64>()
            .map(LiteralValue::Decimal)
            .unwrap_or(LiteralValue::String(content)),
        "http://www.w3.org/2001/XMLSchema#boolean" => match content.as_str() {
            "true" | "1" => LiteralValue::Boolean(true),
            "false" | "0" => LiteralValue::Boolean(false),
            _ => LiteralValue::String(content),
        },
        _ => LiteralValue::String(content),
    }
}

fn as_iri(token: &str) -> Option<&str> {
    if token.starts_with('<') && token.ends_with('>') && token.len() >= 2 {
        Some(&token[1..token.len() - 1])
    } else {
        None
    }
}

fn as_blank(token: &str) -> Option<&str> {
    token.strip_prefix("_:")
}

fn line_err(line_no: usize, message: &'static str) -> OntolithError {
    OntolithError::parse_at(line_no, 1, message)
}
