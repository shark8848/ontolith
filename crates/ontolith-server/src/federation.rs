//! SPARQL 1.1 Federated Query `SERVICE` client (HTTP, Jena/Fuseki parity).
//!
//! Implements [`ontolith_query::domain::ServiceClient`] over the tree's
//! minimal HTTP stack: the inner group pattern (captured verbatim by the
//! parser) is re-submitted as `SELECT * WHERE { … }` against the remote
//! endpoint with an optional VALUES constraint carrying bindings propagated
//! from the enclosing query (dependent dispatch). Responses are decoded with
//! the input-side SPARQL Results parsers (`ontolith-parser::sparql_results`),
//! the mirror image of this server's own result writers, and mapped onto the
//! engine's [`BoundValue`] model via the local dictionary for blank nodes.

use ontolith_core::domain::{Iri, LiteralValue};
use ontolith_core::error::OntolithError;
use ontolith_parser::infrastructure::sparql_results::{
    ResultsTerm, SRJ_MEDIA_TYPE, SparqlResults, parse_results_by_content_type,
};
use ontolith_query::domain::{BoundValue, ServiceClient};
use ontolith_storage::application::DictionaryCodec;
use std::collections::BTreeMap;
use std::sync::Arc;

const FEDERATION_ACCEPT: &str = "application/sparql-results+json, \
    application/sparql-results+xml;q=0.9, text/tab-separated-values;q=0.8, text/csv;q=0.7";

/// SPARQL `SERVICE` over HTTP(S). Only `http://` is implemented (the shared
/// fetch helper rejects `https://` deterministically); `SILENT` handling is
/// the executor's job.
#[derive(Clone)]
pub struct HttpServiceClient {
    dictionary: Arc<dyn DictionaryCodec>,
}

impl HttpServiceClient {
    pub fn new(dictionary: Arc<dyn DictionaryCodec>) -> Self {
        Self { dictionary }
    }
}

impl ServiceClient for HttpServiceClient {
    fn evaluate(
        &self,
        endpoint: &str,
        group_body: &str,
        propagated: &[(String, BoundValue)],
    ) -> Result<Vec<BTreeMap<String, BoundValue>>, OntolithError> {
        let query_text = build_federated_query(group_body, propagated);
        let target = append_query(endpoint, &percent_encode(&query_text));
        let (_status, content_type, body) = crate::jsonld::http_get(&target, FEDERATION_ACCEPT)?;
        let content_type = if content_type.trim().is_empty() {
            SRJ_MEDIA_TYPE.to_owned()
        } else {
            content_type
        };
        let parsed = parse_results_by_content_type(&body, &content_type)?;
        Ok(results_to_rows(parsed, self.dictionary.as_ref()))
    }
}

/// Build the remote `SELECT * WHERE { … }` request body. `propagated` entries
/// are injected as a VALUES clause so the endpoint only returns solutions
/// compatible with the bindings of the enclosing query (dependent dispatch).
pub fn build_federated_query(group_body: &str, propagated: &[(String, BoundValue)]) -> String {
    let mut query = String::from("SELECT * WHERE { ");
    query.push_str(group_body.trim());
    if !propagated.is_empty() {
        let vars: Vec<&str> = propagated.iter().map(|(v, _)| v.as_str()).collect();
        query.push_str(" VALUES (");
        query.push_str(
            &vars
                .iter()
                .map(|v| format!("?{v}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
        query.push_str(") { (");
        let terms: Vec<String> = propagated
            .iter()
            .filter_map(|(_, v)| values_term(v))
            .collect();
        query.push_str(&terms.join(" "));
        query.push_str(") }");
    }
    query.push_str(" }");
    query
}

/// SPARQL VALUES term spelling for a bound value. Blank/node identifiers have
/// no portable lexical form and are skipped (return `None`).
fn values_term(value: &BoundValue) -> Option<String> {
    match value {
        BoundValue::Iri(iri) => Some(format!("<{}>", iri.as_str())),
        BoundValue::Literal(lit) => Some(literal_term(lit)),
        BoundValue::Node(_) | BoundValue::Blank(_) => None,
    }
}

fn literal_term(lit: &LiteralValue) -> String {
    match lit {
        LiteralValue::Lang { value, lang } => {
            format!("\"{}\"@{}", escape_string(value), lang.as_str())
        }
        LiteralValue::String(value) => format!("\"{}\"", escape_string(value)),
        _ => {
            let lexical = escape_string(&lit.lexical_form());
            format!("\"{lexical}\"^^<{}>", lit.xsd_datatype_iri().as_str())
        }
    }
}

fn escape_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// Map decoded result rows onto engine solutions (blank labels are encoded
/// through the local dictionary as `_:label`).
fn results_to_rows(
    parsed: SparqlResults,
    dictionary: &dyn DictionaryCodec,
) -> Vec<BTreeMap<String, BoundValue>> {
    let mut out = Vec::with_capacity(parsed.rows.len());
    for row in parsed.rows {
        let mut solution = BTreeMap::new();
        for (var, term) in row {
            let value = match term {
                ResultsTerm::Iri(s) => BoundValue::Iri(Iri::new(s)),
                ResultsTerm::Literal(l) => BoundValue::Literal(l),
                ResultsTerm::BlankNode(label) => {
                    BoundValue::Blank(dictionary.encode_node(&format!("_:{label}")))
                }
            };
            solution.insert(var, value);
        }
        out.push(solution);
    }
    out
}

/// Append `?query=…` (or `&query=…` when the endpoint already carries a
/// query string) to a SPARQL endpoint URL.
fn append_query(endpoint: &str, encoded_query: &str) -> String {
    let sep = if endpoint.contains('?') { '&' } else { '?' };
    format!("{endpoint}{sep}query={encoded_query}")
}

/// Minimal RFC 3986 percent-encoding for a SPARQL query string value.
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn federated_query_builds_select_and_values_clause() {
        let q = build_federated_query(
            "?o <http://ex.org/name> ?n .",
            &[
                (
                    "o".to_owned(),
                    BoundValue::Iri(Iri::new("http://ex.org/bob")),
                ),
                (
                    "label".to_owned(),
                    BoundValue::Literal(LiteralValue::String("hi \"there\"\n".into())),
                ),
            ],
        );
        assert!(q.starts_with("SELECT * WHERE { ?o <http://ex.org/name> ?n ."));
        assert!(
            q.contains("VALUES (?o ?label) { (<http://ex.org/bob> \"hi \\\"there\\\"\\n\") }"),
            "got: {q}"
        );

        let bare = build_federated_query("?s ?p ?o .", &[]);
        assert_eq!(bare, "SELECT * WHERE { ?s ?p ?o . }");
    }

    #[test]
    fn values_term_spellings() {
        use ontolith_core::domain::LanguageTag;
        assert_eq!(
            values_term(&BoundValue::Iri(Iri::new("http://ex.org/a"))).as_deref(),
            Some("<http://ex.org/a>")
        );
        let lang = LiteralValue::Lang {
            value: "你好".into(),
            lang: LanguageTag::parse("zh").unwrap(),
        };
        assert_eq!(
            values_term(&BoundValue::Literal(lang)).as_deref(),
            Some("\"你好\"@zh")
        );
        let int = BoundValue::Literal(LiteralValue::Integer(5));
        assert_eq!(
            values_term(&int).as_deref(),
            Some("\"5\"^^<http://www.w3.org/2001/XMLSchema#integer>")
        );
        assert_eq!(
            values_term(&BoundValue::Blank(ontolith_core::domain::NodeId::new(1))),
            None
        );
    }

    #[test]
    fn percent_encoding_keeps_only_rfc3986_unreserved() {
        let encoded = percent_encode("SELECT * WHERE { ?s <http://e/p> ?o }");
        assert_eq!(
            encoded,
            "SELECT%20%2A%20WHERE%20%7B%20%3Fs%20%3Chttp%3A%2F%2Fe%2Fp%3E%20%3Fo%20%7D"
        );
        let target = append_query("http://localhost:9999/sparql", &encoded);
        assert!(target.starts_with("http://localhost:9999/sparql?query=SELECT%20"));
        let second = append_query("http://h/sparql?format=json", &encoded);
        assert!(second.starts_with("http://h/sparql?format=json&query="));
    }
}
