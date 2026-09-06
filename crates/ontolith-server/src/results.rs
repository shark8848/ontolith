//! SPARQL Query Results serialization for the HTTP surface (Jena/Fuseki
//! protocol parity): TSV, CSV and SRX (XML), in addition to the JSON writer.
//!
//! Cell encoding follows the same conventions as the W3C-suite result
//! readers in `ontolith-compliance` (IRI `<…>`, quoted literals with
//! `@lang` / `^^<datatype>` suffixes, `_:` blank labels) so clients and
//! conformance harnesses can round-trip the output.

use ontolith_query::domain::{BoundValue, QueryKind, QueryResult};
use ontolith_storage::application::DictionaryCodec;

/// Result-set wire formats served by `/sparql`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultFormat {
    Json,
    Srx,
    Tsv,
    Csv,
}

impl ResultFormat {
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Json => "application/json; charset=utf-8",
            Self::Srx => "application/sparql-results+xml; charset=utf-8",
            Self::Tsv => "text/tab-separated-values; charset=utf-8",
            Self::Csv => "text/csv; charset=utf-8",
        }
    }
}

/// Detect the requested result format from `?format=` / `Accept`.
/// Returns `Json` when nothing matches (the historical default).
pub fn detect_result_format(raw: &str) -> ResultFormat {
    let raw = raw.to_ascii_lowercase();
    if raw.contains("sparql-results+xml") || raw == "srx" || raw == "xml" {
        return ResultFormat::Srx;
    }
    if raw.contains("tab-separated") || raw == "tsv" {
        return ResultFormat::Tsv;
    }
    if raw.contains("text/csv") || raw == "csv" {
        return ResultFormat::Csv;
    }
    ResultFormat::Json
}

/// Whether this result kind can be expressed in a results format (SELECT/ASK).
pub fn is_results_kind(kind: QueryKind) -> bool {
    matches!(kind, QueryKind::Select | QueryKind::Ask)
}

/// SPARQL Query Results TSV Format.
pub fn sparql_results_tsv(result: &QueryResult, dict: &dyn DictionaryCodec) -> String {
    let mut out = String::new();
    if let Some(b) = result.boolean {
        out.push_str(if b { "true" } else { "false" });
        out.push('\n');
        return out;
    }
    if result.kind != QueryKind::Select {
        return out;
    }
    if !result.variables.is_empty() {
        let header = result
            .variables
            .iter()
            .map(|v| format!("?{v}"))
            .collect::<Vec<_>>()
            .join("\t");
        out.push_str(&header);
        out.push('\n');
    }
    for sol in &result.solutions {
        let cells = result
            .variables
            .iter()
            .map(|v| match sol.get(v) {
                Some(bound) => tsv_cell(bound, dict),
                None => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\t");
        out.push_str(&cells);
        out.push('\n');
    }
    out
}

/// SPARQL Query Results CSV Format (RFC 4180 quoting; term cells use the
/// TSV cell convention so IRIs/literals remain distinguishable).
pub fn sparql_results_csv(result: &QueryResult, dict: &dyn DictionaryCodec) -> String {
    let mut out = String::new();
    if let Some(b) = result.boolean {
        out.push_str(if b { "true" } else { "false" });
        out.push('\n');
        return out;
    }
    if result.kind != QueryKind::Select {
        return out;
    }
    if !result.variables.is_empty() {
        let header = result
            .variables
            .iter()
            .map(|v| format!("?{v}"))
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&header);
        out.push('\n');
    }
    for sol in &result.solutions {
        let cells = result
            .variables
            .iter()
            .map(|v| match sol.get(v) {
                Some(bound) => csv_field(&tsv_cell(bound, dict)),
                None => String::new(),
            })
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&cells);
        out.push('\n');
    }
    out
}

/// SPARQL Query Results XML Format (SRX).
pub fn sparql_results_srx(result: &QueryResult, dict: &dyn DictionaryCodec) -> String {
    let mut out = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sparql xmlns="http://www.w3.org/2005/sparql-results#">
"#,
    );
    if let Some(b) = result.boolean {
        out.push_str(if b {
            "  <head/>\n  <boolean>true</boolean>\n"
        } else {
            "  <head/>\n  <boolean>false</boolean>\n"
        });
    } else if result.kind == QueryKind::Select {
        out.push_str("  <head>\n");
        for var in &result.variables {
            out.push_str("    <variable name=\"");
            xml_escape_attr(var, &mut out);
            out.push_str("\"/>\n");
        }
        out.push_str("  </head>\n");
        out.push_str("  <results>\n");
        for sol in &result.solutions {
            out.push_str("    <result>\n");
            for var in &result.variables {
                if let Some(bound) = sol.get(var) {
                    out.push_str("      <binding name=\"");
                    xml_escape_attr(var, &mut out);
                    out.push_str("\">");
                    push_srx_term(bound, dict, &mut out);
                    out.push_str("</binding>\n");
                }
            }
            out.push_str("    </result>\n");
        }
        out.push_str("  </results>\n");
    } else {
        out.push_str("  <head/>\n");
    }
    out.push_str("</sparql>\n");
    out
}

fn tsv_cell(bound: &BoundValue, dict: &dyn DictionaryCodec) -> String {
    match bound {
        BoundValue::Iri(iri) => format!("<{}>", iri.as_str()),
        BoundValue::Blank(id) => format!("_:{}", bnode_label(*id, dict)),
        BoundValue::Node(id) => match dict.decode_node(*id) {
            Some(value) if !value.starts_with("_:") => format!("<{}>", value),
            _ => format!("_:{}", bnode_label(*id, dict)),
        },
        BoundValue::Literal(lit) => {
            let lex = lit.lexical_form();
            let escaped = lex
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\t', "\\t")
                .replace('\n', "\\n")
                .replace('\r', "\\r");
            if let Some(lang) = lit.language_tag() {
                format!("\"{escaped}\"@{}", lang.as_str())
            } else if lit.xsd_datatype_iri().as_str() == "http://www.w3.org/2001/XMLSchema#string" {
                format!("\"{escaped}\"")
            } else {
                format!("\"{escaped}\"^^<{}>", lit.xsd_datatype_iri().as_str())
            }
        }
    }
}

fn csv_field(cell: &str) -> String {
    if cell.contains(',') || cell.contains('"') || cell.contains('\n') || cell.contains('\r') {
        format!("\"{}\"", cell.replace('"', "\"\""))
    } else {
        cell.to_owned()
    }
}

fn push_srx_term(bound: &BoundValue, dict: &dyn DictionaryCodec, out: &mut String) {
    match bound {
        BoundValue::Iri(iri) => {
            out.push_str("<uri>");
            xml_escape_text(iri.as_str(), out);
            out.push_str("</uri>");
        }
        BoundValue::Blank(id) => {
            out.push_str("<bnode>");
            xml_escape_text(&bnode_label(*id, dict), out);
            out.push_str("</bnode>");
        }
        BoundValue::Node(id) => match dict.decode_node(*id) {
            Some(value) if !value.starts_with("_:") => {
                out.push_str("<uri>");
                xml_escape_text(&value, out);
                out.push_str("</uri>");
            }
            _ => {
                out.push_str("<bnode>");
                xml_escape_text(&bnode_label(*id, dict), out);
                out.push_str("</bnode>");
            }
        },
        BoundValue::Literal(lit) => {
            out.push_str("<literal");
            if let Some(lang) = lit.language_tag() {
                out.push_str(" xml:lang=\"");
                xml_escape_attr(lang.as_str(), out);
                out.push('"');
            } else if lit.xsd_datatype_iri().as_str() != "http://www.w3.org/2001/XMLSchema#string" {
                out.push_str(" datatype=\"");
                xml_escape_attr(lit.xsd_datatype_iri().as_str(), out);
                out.push('"');
            }
            out.push('>');
            xml_escape_text(&lit.lexical_form(), out);
            out.push_str("</literal>");
        }
    }
}

fn bnode_label(id: ontolith_core::domain::NodeId, dict: &dyn DictionaryCodec) -> String {
    match dict.decode_node(id) {
        Some(value) if value.starts_with("_:") => value.trim_start_matches("_:").to_owned(),
        _ => format!("n{}", id.get()),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_core::domain::{LanguageTag, LiteralValue};
    use ontolith_query::domain::{QueryKind, Solution};
    use ontolith_storage::infrastructure::InMemoryDictionary;

    fn select_result(dict: &InMemoryDictionary) -> QueryResult {
        let alice = dict.encode_node("urn:alice");
        let mut sol = Solution::new();
        sol.insert("person", BoundValue::Node(alice));
        sol.insert(
            "name",
            BoundValue::Literal(LiteralValue::Lang {
                value: "Alice \"A\"".into(),
                lang: LanguageTag::parse("en").unwrap(),
            }),
        );
        QueryResult {
            kind: QueryKind::Select,
            variables: vec!["person".into(), "name".into()],
            solutions: vec![sol],
            boolean: None,
            construct_triples: Vec::new(),
            affected: 0,
            elapsed_ms: 1,
            timed_out: false,
            cancelled: false,
        }
    }

    #[test]
    fn tsv_header_rows_and_escaping() {
        let dict = InMemoryDictionary::new();
        let tsv = sparql_results_tsv(&select_result(&dict), &dict);
        let lines: Vec<&str> = tsv.lines().collect();
        assert_eq!(lines[0], "?person\t?name");
        assert!(lines[1].contains("<urn:alice>"));
        assert!(lines[1].contains("\"Alice \\\"A\\\"\"@en"));
    }

    #[test]
    fn csv_quotes_comma_and_quote_cells() {
        let dict = InMemoryDictionary::new();
        let csv = sparql_results_csv(&select_result(&dict), &dict);
        assert!(csv.starts_with("?person,?name\n"));
        assert!(csv.contains("Alice"));
        assert!(csv.contains("@en"));
    }

    #[test]
    fn srx_contains_uri_and_literal_elements() {
        let dict = InMemoryDictionary::new();
        let xml = sparql_results_srx(&select_result(&dict), &dict);
        assert!(xml.contains("<variable name=\"person\"/>"));
        assert!(xml.contains("<uri>urn:alice</uri>"));
        assert!(xml.contains("<literal xml:lang=\"en\">Alice \"A\"</literal>"));
    }

    #[test]
    fn ask_boolean_formats() {
        let dict = InMemoryDictionary::new();
        let mut ask = QueryResult {
            kind: QueryKind::Ask,
            variables: Vec::new(),
            solutions: Vec::new(),
            boolean: Some(true),
            construct_triples: Vec::new(),
            affected: 0,
            elapsed_ms: 1,
            timed_out: false,
            cancelled: false,
        };
        assert_eq!(sparql_results_tsv(&ask, &dict), "true\n");
        assert_eq!(sparql_results_csv(&ask, &dict), "true\n");
        assert!(sparql_results_srx(&ask, &dict).contains("<boolean>true</boolean>"));

        ask.boolean = Some(false);
        assert_eq!(sparql_results_tsv(&ask, &dict), "false\n");
    }

    #[test]
    fn detect_prefers_specific_types() {
        assert_eq!(detect_result_format("tsv"), ResultFormat::Tsv);
        assert_eq!(
            detect_result_format("application/sparql-results+xml"),
            ResultFormat::Srx
        );
        assert_eq!(detect_result_format("text/csv"), ResultFormat::Csv);
        assert_eq!(detect_result_format("*/*"), ResultFormat::Json);
    }
}
