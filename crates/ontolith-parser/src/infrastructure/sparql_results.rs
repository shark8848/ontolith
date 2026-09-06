//! SPARQL Query Results **input** parsing (Jena/Fuseki protocol parity, input
//! side): SRJ (JSON), SRX (XML), TSV and CSV result documents are decoded
//! into ordered solution rows of document-level RDF terms.
//!
//! The decoders are the mirror image of the output writers in
//! `ontolith-server::results` and follow the same cell conventions as the
//! W3C-suite result readers in `ontolith-compliance`, so our own output
//! round-trips byte-for-byte through these parsers.

use crate::infrastructure::term_lex::{coerce_typed_literal, unescape_string};
use ontolith_core::domain::{LanguageTag, LiteralValue};
use ontolith_core::error::OntolithError;
use std::collections::BTreeMap;

/// Wire media types spoken by SPARQL endpoints for query results.
pub const SRJ_MEDIA_TYPE: &str = "application/sparql-results+json";
pub const SRX_MEDIA_TYPE: &str = "application/sparql-results+xml";
pub const TSV_MEDIA_TYPE: &str = "text/tab-separated-values";
pub const CSV_MEDIA_TYPE: &str = "text/csv";

/// SPARQL result-set document formats accepted on the input side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultsFormat {
    Srj,
    Srx,
    Tsv,
    Csv,
}

impl ResultsFormat {
    pub fn from_content_type(raw: &str) -> Option<Self> {
        let raw = raw.to_ascii_lowercase();
        if raw.contains("sparql-results+xml") {
            Some(Self::Srx)
        } else if raw.contains("sparql-results+json") || raw.contains("json") {
            Some(Self::Srj)
        } else if raw.contains("tab-separated") {
            Some(Self::Tsv)
        } else if raw.contains("csv") {
            Some(Self::Csv)
        } else {
            None
        }
    }
}

/// An RDF term as carried by a SPARQL results document. IRI and blank-node
/// payloads are kept as strings so the parser never touches a local
/// dictionary; literals reuse the core value model.
#[derive(Debug, Clone, PartialEq)]
pub enum ResultsTerm {
    Iri(String),
    /// Blank node label, without any leading `_:` prefix.
    BlankNode(String),
    Literal(LiteralValue),
}

/// A decoded SPARQL query-results document.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SparqlResults {
    /// Projection order from the document head (SELECT) or empty (ASK).
    pub variables: Vec<String>,
    /// Ordered solution rows. An unbound variable is simply absent from its
    /// row map.
    pub rows: Vec<BTreeMap<String, ResultsTerm>>,
    /// ASK outcome when the document is a boolean result set.
    pub boolean: Option<bool>,
}

/// Parse a SPARQL results document of the given format.
pub fn parse_results_document(
    text: &str,
    format: ResultsFormat,
) -> Result<SparqlResults, OntolithError> {
    match format {
        ResultsFormat::Srj => parse_srj(text),
        ResultsFormat::Srx => parse_srx(text),
        ResultsFormat::Tsv => parse_tsv(text),
        ResultsFormat::Csv => parse_csv(text),
    }
}

/// Dispatch on a response `Content-Type` header value.
pub fn parse_results_by_content_type(
    text: &str,
    content_type: &str,
) -> Result<SparqlResults, OntolithError> {
    let format = ResultsFormat::from_content_type(content_type).ok_or_else(|| {
        OntolithError::failed(format!(
            "unsupported SPARQL results content type: {content_type}"
        ))
    })?;
    parse_results_document(text, format)
}

/// SPARQL 1.1 Query Results JSON Format (`application/sparql-results+json`).
pub fn parse_srj(text: &str) -> Result<SparqlResults, OntolithError> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| OntolithError::failed(format!("invalid SRJ document: {e}")))?;
    let mut out = SparqlResults::default();

    if let Some(vars) = value["head"]["vars"].as_array() {
        for v in vars {
            if let Some(name) = v.as_str() {
                out.variables.push(name.to_owned());
            }
        }
    }
    if let Some(boolean) = value["boolean"].as_bool() {
        out.boolean = Some(boolean);
        return Ok(out);
    }
    let bindings = value["results"]["bindings"].as_array();
    if bindings.is_none() {
        return Err(OntolithError::InvalidArgument(
            "SRJ document has neither a boolean nor results.bindings",
        ));
    }
    for binding in bindings.unwrap() {
        let Some(object) = binding.as_object() else {
            continue;
        };
        let mut row = BTreeMap::new();
        for (var, term) in object {
            let Some(term) = parse_srj_term(term)? else {
                continue;
            };
            row.insert(var.clone(), term);
        }
        out.rows.push(row);
    }
    Ok(out)
}

fn parse_srj_term(value: &serde_json::Value) -> Result<Option<ResultsTerm>, OntolithError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let kind = object.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let raw = object.get("value").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "uri" | "iri" => Ok(Some(ResultsTerm::Iri(raw.to_owned()))),
        "bnode" | "blank" => Ok(Some(ResultsTerm::BlankNode(
            raw.strip_prefix("_:").unwrap_or(raw).to_owned(),
        ))),
        "literal" | "typed-literal" => {
            let datatype = object.get("datatype").and_then(|v| v.as_str());
            let lang = object
                .get("xml:lang")
                .and_then(|v| v.as_str())
                .or_else(|| object.get("lang").and_then(|v| v.as_str()));
            Ok(Some(parse_literal(raw, datatype, lang)?))
        }
        _ => Ok(None),
    }
}

/// SPARQL 1.1 Query Results XML Format (`application/sparql-results+xml`).
pub fn parse_srx(text: &str) -> Result<SparqlResults, OntolithError> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut out = SparqlResults::default();
    let mut row: Option<BTreeMap<String, ResultsTerm>> = None;
    let mut current_var: Option<String> = None;
    let mut term_kind: Option<&'static str> = None;
    let mut datatype: Option<String> = None;
    let mut lang: Option<String> = None;
    let mut text_buf = String::new();

    let invalid = |msg: &str| OntolithError::failed(format!("invalid SRX document: {msg}"));

    loop {
        match reader.read_event() {
            Err(e) => return Err(invalid(&format!("xml error: {e}"))),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"variable" => {
                    if let Some(name) = attr(&e, b"name") {
                        out.variables.push(name);
                    }
                }
                b"result" => row = Some(BTreeMap::new()),
                b"binding" => current_var = attr(&e, b"name"),
                b"uri" => term_kind = Some("uri"),
                b"bnode" => term_kind = Some("bnode"),
                b"literal" => {
                    term_kind = Some("literal");
                    datatype = attr(&e, b"datatype");
                    lang = attr(&e, b"xml:lang").or_else(|| attr(&e, b"lang"));
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"variable"
                    && let Some(name) = attr(&e, b"name")
                {
                    out.variables.push(name);
                }
            }
            Ok(Event::Text(t)) => {
                if let Ok(decoded) = t.unescape() {
                    text_buf.push_str(&decoded);
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"uri" | b"bnode" | b"literal" => {
                    let term = match term_kind {
                        Some("uri") => ResultsTerm::Iri(text_buf.clone()),
                        Some("bnode") => {
                            ResultsTerm::BlankNode(text_buf.trim_start_matches("_:").to_owned())
                        }
                        Some("literal") => {
                            parse_literal(&text_buf, datatype.as_deref(), lang.as_deref())?
                        }
                        _ => ResultsTerm::Literal(LiteralValue::String(text_buf.clone())),
                    };
                    if let (Some(row), Some(var)) = (&mut row, &current_var) {
                        row.insert(var.clone(), term);
                    }
                    text_buf.clear();
                    term_kind = None;
                    datatype = None;
                    lang = None;
                }
                b"binding" => current_var = None,
                b"result" => {
                    if let Some(r) = row.take() {
                        out.rows.push(r);
                    }
                }
                b"boolean" => {
                    out.boolean = Some(text_buf.trim() == "true");
                    text_buf.clear();
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok(out)
}

/// SPARQL 1.1 Query Results TSV Format (`text/tab-separated-values`).
pub fn parse_tsv(text: &str) -> Result<SparqlResults, OntolithError> {
    let mut out = SparqlResults::default();
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Ok(out);
    }
    // A single bare boolean line is the TSV encoding of an ASK result.
    let trimmed: Vec<&str> = lines.iter().map(|l| l.trim()).collect();
    if trimmed.len() == 1
        && lines.iter().all(|l| !l.contains('\t'))
        && matches!(trimmed[0], "true" | "false")
    {
        out.boolean = Some(trimmed[0] == "true");
        return Ok(out);
    }
    let header = lines
        .first()
        .ok_or(OntolithError::InvalidArgument("empty TSV results document"))?;
    let variables: Vec<String> = header
        .split('\t')
        .map(|v| v.trim().trim_start_matches('?').to_owned())
        .collect();
    out.variables = variables;
    for line in lines.iter().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let cells: Vec<&str> = line.split('\t').collect();
        if cells.len() != out.variables.len() {
            return Err(OntolithError::failed(format!(
                "TSV row arity mismatch: expected {}, got {}",
                out.variables.len(),
                cells.len()
            )));
        }
        let mut row = BTreeMap::new();
        for (var, cell) in out.variables.iter().zip(cells) {
            let cell = cell.trim();
            if cell.is_empty() {
                continue;
            }
            row.insert(var.clone(), parse_tsv_cell(cell)?);
        }
        out.rows.push(row);
    }
    Ok(out)
}

/// SPARQL 1.1 Query Results CSV Format (`text/csv`; RFC 4180 quoting).
///
/// Cells use the TSV cell convention (IRI `<…>`, quoted literals, `_:`
/// blanks) so documents written by `ontolith-server`'s CSV writer and the
/// plain W3C bare-lexical spelling both decode.
pub fn parse_csv(text: &str) -> Result<SparqlResults, OntolithError> {
    let table = split_csv(text)?;
    let mut records = table
        .into_iter()
        .filter(|r| !r.iter().all(|c| c.trim().is_empty()));
    let header = records
        .next()
        .ok_or(OntolithError::InvalidArgument("empty CSV results document"))?;
    let variables: Vec<String> = header
        .iter()
        .map(|v| v.trim().trim_start_matches('?').to_owned())
        .collect();
    let mut out = SparqlResults {
        variables,
        ..Default::default()
    };
    for record in records {
        let mut row = BTreeMap::new();
        for (idx, cell) in record.iter().enumerate() {
            let cell = cell.trim();
            if cell.is_empty() || idx >= out.variables.len() {
                continue;
            }
            row.insert(out.variables[idx].clone(), parse_tsv_cell(cell)?);
        }
        out.rows.push(row);
    }
    Ok(out)
}

/// RFC 4180-ish CSV table splitter (quoted fields, `""` escapes, CRLF).
fn split_csv(text: &str) -> Result<Vec<Vec<String>>, OntolithError> {
    let mut table: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_quotes {
            if c == '"' {
                if i + 1 < chars.len() && chars[i + 1] == '"' {
                    field.push('"');
                    i += 1;
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => {
                    current.push(std::mem::take(&mut field));
                }
                '\n' => {
                    current.push(std::mem::take(&mut field));
                    table.push(std::mem::take(&mut current));
                }
                '\r' => {}
                _ => field.push(c),
            }
        }
        i += 1;
    }
    if !field.is_empty() || !current.is_empty() {
        current.push(field);
        table.push(current);
    }
    Ok(table)
}

/// Decode one TSV cell into an RDF term (IRI, blank node or literal).
fn parse_tsv_cell(cell: &str) -> Result<ResultsTerm, OntolithError> {
    let invalid = || OntolithError::failed(format!("invalid TSV result cell: {cell}"));
    if let Some(inner) = cell.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        return Ok(ResultsTerm::Iri(inner.to_owned()));
    }
    if let Some(label) = cell.strip_prefix("_:") {
        return Ok(ResultsTerm::BlankNode(label.to_owned()));
    }
    if let Some(rest) = cell.strip_prefix('"') {
        // Turtle-style quoted literal: scan to the closing quote honouring
        // backslash escapes (`\\`, `\"` …).
        let mut lex = String::new();
        let mut chars = rest.chars().peekable();
        loop {
            match chars.next() {
                None => return Err(invalid()),
                Some('\\') => match chars.next() {
                    Some(next) => {
                        lex.push('\\');
                        lex.push(next);
                    }
                    None => return Err(invalid()),
                },
                Some('"') => break,
                Some(c) => lex.push(c),
            }
        }
        let suffix: String = chars.collect();
        let suffix = suffix.trim();
        if let Some(l) = suffix.strip_prefix('@') {
            let lang = LanguageTag::parse(l)?;
            return Ok(ResultsTerm::Literal(LiteralValue::Lang {
                value: unescape_string(&lex),
                lang,
            }));
        }
        if let Some(dt) = suffix.strip_prefix("^^<").and_then(|s| s.strip_suffix('>')) {
            return Ok(ResultsTerm::Literal(coerce_typed_literal(
                unescape_string(&lex),
                dt,
            )));
        }
        if suffix.is_empty() {
            return Ok(ResultsTerm::Literal(LiteralValue::String(unescape_string(
                &lex,
            ))));
        }
        return Err(invalid());
    }
    // Unquoted cell: numeric / boolean lexical form per the TSV spec.
    if let Ok(i) = cell.parse::<i64>() {
        return Ok(ResultsTerm::Literal(LiteralValue::Integer(i)));
    }
    if cell == "true" || cell == "false" {
        return Ok(ResultsTerm::Literal(LiteralValue::Boolean(cell == "true")));
    }
    if let Ok(d) = cell.parse::<f64>() {
        if cell.contains('e') || cell.contains('E') {
            return Ok(ResultsTerm::Literal(LiteralValue::Double(d)));
        }
        return Ok(ResultsTerm::Literal(LiteralValue::Decimal(d)));
    }
    Ok(ResultsTerm::Literal(LiteralValue::String(cell.to_owned())))
}

/// Build a literal from a raw lexical form plus optional datatype/lang.
fn parse_literal(
    lexical: &str,
    datatype: Option<&str>,
    lang: Option<&str>,
) -> Result<ResultsTerm, OntolithError> {
    if let Some(l) = lang {
        return Ok(ResultsTerm::Literal(LiteralValue::Lang {
            value: lexical.to_owned(),
            lang: LanguageTag::parse(l)?,
        }));
    }
    match datatype {
        Some(dt) if !dt.is_empty() => Ok(ResultsTerm::Literal(coerce_typed_literal(
            lexical.to_owned(),
            dt,
        ))),
        _ => Ok(ResultsTerm::Literal(LiteralValue::String(
            lexical.to_owned(),
        ))),
    }
}

fn attr(e: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Option<String> {
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.as_ref() == name)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term_iri(s: &str) -> ResultsTerm {
        ResultsTerm::Iri(s.to_owned())
    }

    #[test]
    fn srj_select_rows_and_ask_round_trip() {
        let doc = r#"{
  "head": { "vars": [ "s", "o" ] },
  "results": { "bindings": [
    { "s": { "type": "uri", "value": "http://ex.org/alice" },
      "o": { "type": "literal", "value": "hello", "xml:lang": "en" } },
    { "s": { "type": "bnode", "value": "_:b0" },
      "o": { "type": "literal", "datatype": "http://www.w3.org/2001/XMLSchema#integer", "value": "5" } }
  ] }
}"#;
        let parsed = parse_srj(doc).unwrap();
        assert_eq!(parsed.variables, vec!["s", "o"]);
        assert_eq!(parsed.boolean, None);
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(
            parsed.rows[0].get("s"),
            Some(&term_iri("http://ex.org/alice"))
        );
        assert!(matches!(
            parsed.rows[0].get("o"),
            Some(ResultsTerm::Literal(LiteralValue::Lang { lang, .. }))
                if lang.as_str() == "en"
        ));
        assert_eq!(
            parsed.rows[1].get("s"),
            Some(&ResultsTerm::BlankNode("b0".into()))
        );
        assert_eq!(
            parsed.rows[1].get("o"),
            Some(&ResultsTerm::Literal(LiteralValue::Integer(5)))
        );

        let ask = parse_srj(r#"{ "head": {}, "boolean": true }"#).unwrap();
        assert_eq!(ask.boolean, Some(true));
        assert!(ask.rows.is_empty());
    }

    #[test]
    fn srx_select_and_ask_parse() {
        let doc = r#"<?xml version="1.0"?>
<sparql xmlns="http://www.w3.org/2005/sparql-results#">
  <head><variable name="s"/><variable name="o"/></head>
  <results>
    <result>
      <binding name="s"><uri>http://ex.org/x</uri></binding>
      <binding name="o"><literal xml:lang="zh">你好</literal></binding>
    </result>
    <result>
      <binding name="s"><bnode>b9</bnode></binding>
      <binding name="o"><literal datatype="http://www.w3.org/2001/XMLSchema#double">1.5</literal></binding>
    </result>
  </results>
</sparql>"#;
        let parsed = parse_srx(doc).unwrap();
        assert_eq!(parsed.variables, vec!["s", "o"]);
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.rows[0].get("s"), Some(&term_iri("http://ex.org/x")));
        assert!(matches!(
            parsed.rows[0].get("o"),
            Some(ResultsTerm::Literal(LiteralValue::Lang { value, lang }))
                if value == "你好" && lang.as_str() == "zh"
        ));
        assert_eq!(
            parsed.rows[1].get("s"),
            Some(&ResultsTerm::BlankNode("b9".into()))
        );
        assert!(matches!(
            parsed.rows[1].get("o"),
            Some(ResultsTerm::Literal(LiteralValue::Double(_)))
        ));

        let ask = parse_srx(r#"<sparql xmlns="http://www.w3.org/2005/sparql-results#"><head/><boolean>false</boolean></sparql>"#)
            .unwrap();
        assert_eq!(ask.boolean, Some(false));
    }

    #[test]
    fn tsv_select_ask_and_cell_forms() {
        let doc = "?s\t?o\n<http://ex.org/a>\t\"hi\"@en\n_:b1\t\"5\"^^<http://www.w3.org/2001/XMLSchema#integer>\n<http://ex.org/c>\t\"line\\nbreak\"\n";
        let parsed = parse_tsv(doc).unwrap();
        assert_eq!(parsed.variables, vec!["s", "o"]);
        assert_eq!(parsed.rows.len(), 3);
        assert_eq!(parsed.rows[0].get("s"), Some(&term_iri("http://ex.org/a")));
        assert!(matches!(
            parsed.rows[0].get("o"),
            Some(ResultsTerm::Literal(LiteralValue::Lang { lang, .. }))
                if lang.as_str() == "en"
        ));
        assert_eq!(
            parsed.rows[1].get("s"),
            Some(&ResultsTerm::BlankNode("b1".into()))
        );
        assert_eq!(
            parsed.rows[1].get("o"),
            Some(&ResultsTerm::Literal(LiteralValue::Integer(5)))
        );
        assert!(matches!(
            parsed.rows[2].get("o"),
            Some(ResultsTerm::Literal(LiteralValue::String(v))) if v == "line\nbreak"
        ));
        // Unbound object cell must stay absent.
        let unbound = parse_tsv("?a\t?b\n<http://ex.org/a>\t\n").unwrap();
        assert_eq!(unbound.rows[0].len(), 1);
        assert!(!unbound.rows[0].contains_key("b"));

        let ask = parse_tsv("true\n").unwrap();
        assert_eq!(ask.boolean, Some(true));
    }

    #[test]
    fn csv_parses_quoted_and_tsv_cells() {
        // Writer-side convention (IRI `<…>`, quoted literals) plus plain
        // W3C bare-lexical cells.
        let doc = "s,o\n<http://ex.org/a>,\"4,4\"\nhttp://ex.org/b,true\n_:x,7\n";
        let parsed = parse_csv(doc).unwrap();
        assert_eq!(parsed.variables, vec!["s", "o"]);
        assert_eq!(parsed.rows.len(), 3);
        assert_eq!(parsed.rows[0].get("s"), Some(&term_iri("http://ex.org/a")));
        assert!(matches!(
            parsed.rows[0].get("o"),
            Some(ResultsTerm::Literal(LiteralValue::String(v))) if v == "4,4"
        ));
        assert!(matches!(
            parsed.rows[1].get("o"),
            Some(ResultsTerm::Literal(LiteralValue::Boolean(true)))
        ));
        assert_eq!(
            parsed.rows[2].get("s"),
            Some(&ResultsTerm::BlankNode("x".into()))
        );
        assert_eq!(
            parsed.rows[2].get("o"),
            Some(&ResultsTerm::Literal(LiteralValue::Integer(7)))
        );
    }

    #[test]
    fn content_type_dispatch_and_errors() {
        let json = parse_results_by_content_type(
            r#"{ "head": { "vars": [] }, "results": { "bindings": [] } }"#,
            "application/sparql-results+json; charset=utf-8",
        )
        .unwrap();
        assert!(json.rows.is_empty());

        let err = parse_srj("{ not json").unwrap_err();
        assert!(err.message().contains("invalid SRJ"));
        let err = parse_tsv("?a\t?b\n<http://e/a>\t<http://e/b>\textra\n").unwrap_err();
        assert!(err.message().contains("arity mismatch"));
        let unsupported =
            parse_results_by_content_type("{}", "application/octet-stream").unwrap_err();
        assert!(unsupported.message().contains("unsupported"));
    }
}
