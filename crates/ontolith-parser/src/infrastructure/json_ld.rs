//! JSON-LD parser (WBS-02 JSON-LD 导入): pragmatic RDF 1.0/1.1 subset.
//!
//! Supported:
//! - `@context`: object form (term → IRI strings; term definitions with
//!   `@id`/`@type` (`@id`|`@vocab`|datatype)/`@language`/`@container`
//!   (`@list`|`@set`|`@language`|`@index`)), `@vocab`, `@base`,
//!   `@language`, keyword aliases, array of contexts (merged left-to-right).
//! - Node objects: `@id` (IRI / `_:` blank label), `@type` (string|array),
//!   properties with string/number/boolean/object/array/null values, nested
//!   node objects (minted blank nodes), value objects (`@value` with
//!   `@language`/`@type`), `@list`, `@set`, node-level `@language`,
//!   `{"@id": g, "@graph": [...]}` named graphs (quads).
//! - Term/prefix expansion, base IRI resolution.
//!
//! Not supported (explicit errors): remote `@context` URLs, `@reverse`,
//! `@nest`, `@included`, `@json`.

use crate::domain::{RdfEvent, RdfEventSink};
use crate::infrastructure::term_lex::resolve_against_base;
use ontolith_core::domain::{Iri, LanguageTag, LiteralValue, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Quad, Term, Triple};
use ontolith_storage::application::DictionaryCodec;
use serde_json::Value;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";

#[derive(Debug, Clone, Default)]
struct TermDef {
    id: Option<String>,
    type_: Option<TypeMapping>,
    language: Option<String>,
    container: Container,
}

#[derive(Debug, Clone, PartialEq)]
enum TypeMapping {
    Id,
    Vocab,
    Datatype(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Container {
    #[default]
    None,
    List,
    Set,
    Language,
    Index,
}

#[derive(Debug, Clone, Default)]
struct Context {
    terms: std::collections::HashMap<String, TermDef>,
    vocab: Option<String>,
    base: Option<String>,
    default_language: Option<String>,
}

fn parse_error(message: impl AsRef<str>) -> OntolithError {
    OntolithError::parse_at(0, 0, message)
}

/// RDF 1.1 literal coercion for known XSD datatypes (mirrors the Turtle/NT
/// path so query results round-trip identically).
fn coerce_literal(content: String, datatype: &str) -> LiteralValue {
    match datatype {
        "http://www.w3.org/2001/XMLSchema#string" => LiteralValue::String(content),
        "http://www.w3.org/2001/XMLSchema#integer" => content
            .parse::<i64>()
            .map(LiteralValue::Integer)
            .unwrap_or_else(|_| LiteralValue::Typed {
                value: content,
                datatype: Iri::new(datatype),
            }),
        "http://www.w3.org/2001/XMLSchema#decimal" => content
            .parse::<f64>()
            .map(LiteralValue::Decimal)
            .unwrap_or_else(|_| LiteralValue::Typed {
                value: content,
                datatype: Iri::new(datatype),
            }),
        "http://www.w3.org/2001/XMLSchema#float" => content
            .parse::<f32>()
            .map(LiteralValue::Float)
            .unwrap_or_else(|_| LiteralValue::Typed {
                value: content,
                datatype: Iri::new(datatype),
            }),
        "http://www.w3.org/2001/XMLSchema#double" => content
            .parse::<f64>()
            .map(LiteralValue::Double)
            .unwrap_or_else(|_| LiteralValue::Typed {
                value: content,
                datatype: Iri::new(datatype),
            }),
        "http://www.w3.org/2001/XMLSchema#boolean" => match content.as_str() {
            "true" => LiteralValue::Boolean(true),
            "false" => LiteralValue::Boolean(false),
            _ => LiteralValue::Typed {
                value: content,
                datatype: Iri::new(datatype),
            },
        },
        _ => LiteralValue::Typed {
            value: content,
            datatype: Iri::new(datatype),
        },
    }
}

fn expand_iri_ref(ctx: &Context, value: &str, vocab: bool) -> String {
    if value.starts_with("_:") || value.starts_with('@') {
        return value.to_owned();
    }
    if let Some((prefix, rest)) = value.split_once(':') {
        if let Some(def) = ctx.terms.get(prefix)
            && let Some(id) = &def.id
            && !id.starts_with('@')
            && !rest.is_empty()
        {
            return format!("{id}{rest}");
        }
        return value.to_owned();
    }
    if vocab && let Some(v) = &ctx.vocab {
        return format!("{v}{value}");
    }
    if let Some(base) = &ctx.base {
        return resolve_against_base(base, value);
    }
    value.to_owned()
}

fn literal_from_value(value: &Value) -> Option<LiteralValue> {
    match value {
        Value::String(s) => Some(LiteralValue::String(s.clone())),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(LiteralValue::Integer(i))
            } else {
                Some(LiteralValue::Double(n.as_f64().unwrap_or(0.0)))
            }
        }
        Value::Bool(b) => Some(LiteralValue::Boolean(*b)),
        _ => None,
    }
}

pub struct JsonLdParser<'a> {
    context: Context,
    dictionary: &'a dyn DictionaryCodec,
    blank_count: usize,
}

impl<'a> JsonLdParser<'a> {
    pub fn new(dictionary: &'a dyn DictionaryCodec, base_iri: Option<String>) -> Self {
        Self {
            context: Context {
                base: base_iri,
                ..Context::default()
            },
            dictionary,
            blank_count: 0,
        }
    }

    pub fn parse_into(
        &mut self,
        input: &str,
        sink: &mut dyn RdfEventSink,
    ) -> Result<(), OntolithError> {
        let doc: Value =
            serde_json::from_str(input).map_err(|e| parse_error(format!("invalid JSON: {e}")))?;
        match doc {
            Value::Array(nodes) => {
                for node in nodes {
                    let _ = self.node_object(&node, None, sink)?;
                }
                Ok(())
            }
            Value::Object(map) => {
                if let Some(ctx) = map.get("@context") {
                    self.apply_context(ctx)?;
                }
                if let Some(graph) = map.get("@graph") {
                    let named = map
                        .get("@id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.starts_with("_:"))
                        .map(|id| Iri::new(expand_iri_ref(&self.context, id, false)));
                    self.process_graph(graph, named, sink)?;
                } else {
                    let _ = self.node_object(&Value::Object(map), None, sink)?;
                }
                Ok(())
            }
            _ => Err(parse_error("JSON-LD document must be an object or array")),
        }
    }

    fn apply_context(&mut self, value: &Value) -> Result<(), OntolithError> {
        match value {
            Value::Null => {
                self.context = Context {
                    base: self.context.base.clone(),
                    ..Context::default()
                };
                Ok(())
            }
            Value::Array(items) => {
                for item in items {
                    self.apply_context(item)?;
                }
                Ok(())
            }
            Value::String(_) => Err(OntolithError::Unsupported("json-ld remote @context URL")),
            Value::Object(map) => {
                // Pass 1: @vocab / @base / @language and raw string term
                // definitions (prefixes), so pass 2 can expand prefixed IRIs.
                for (key, val) in map {
                    match key.as_str() {
                        "@vocab" => {
                            let v = val
                                .as_str()
                                .ok_or_else(|| parse_error("@vocab must be a string"))?;
                            self.context.vocab = Some(expand_iri_ref(&self.context, v, false));
                        }
                        "@base" => {
                            let v = val
                                .as_str()
                                .ok_or_else(|| parse_error("@base must be a string"))?;
                            self.context.base = Some(expand_iri_ref(&self.context, v, false));
                        }
                        "@language" => {
                            if let Value::String(s) = val {
                                self.context.default_language = Some(s.clone());
                            } else {
                                self.context.default_language = None;
                            }
                        }
                        _ => {}
                    }
                }
                for (key, val) in map {
                    if key.starts_with('@') {
                        continue;
                    }
                    if let Value::String(s) = val {
                        self.context.terms.insert(
                            key.clone(),
                            TermDef {
                                id: Some(expand_iri_ref(&self.context, s, true)),
                                ..TermDef::default()
                            },
                        );
                    }
                }
                // Pass 2: full term definitions.
                for (key, val) in map {
                    if key.starts_with('@') {
                        continue;
                    }
                    if !val.is_string() {
                        let def = self.parse_term_def(key, val)?;
                        self.context.terms.insert(key.clone(), def);
                    }
                }
                Ok(())
            }
            _ => Err(parse_error("@context must be an object, array, or null")),
        }
    }

    fn parse_term_def(&self, key: &str, val: &Value) -> Result<TermDef, OntolithError> {
        let _ = key;
        match val {
            Value::Null => Ok(TermDef::default()),
            Value::Object(map) => {
                let mut def = TermDef::default();
                for (k, v) in map {
                    match k.as_str() {
                        "@id" => {
                            if let Value::String(s) = v {
                                def.id = Some(expand_iri_ref(&self.context, s, true));
                            } else if v.is_null() {
                                def.id = None;
                            } else {
                                return Err(parse_error("term @id must be a string or null"));
                            }
                        }
                        "@type" => {
                            let s = v
                                .as_str()
                                .ok_or_else(|| parse_error("term @type must be a string"))?;
                            def.type_ = Some(match s {
                                "@id" => TypeMapping::Id,
                                "@vocab" => TypeMapping::Vocab,
                                other => TypeMapping::Datatype(expand_iri_ref(
                                    &self.context,
                                    other,
                                    true,
                                )),
                            });
                        }
                        "@language" => {
                            if let Value::String(s) = v {
                                def.language = Some(s.clone());
                            }
                        }
                        "@container" => {
                            def.container = match v.as_str() {
                                Some("@list") => Container::List,
                                Some("@set") => Container::Set,
                                Some("@language") => Container::Language,
                                Some("@index") => Container::Index,
                                _ => Container::None,
                            };
                        }
                        _ => {}
                    }
                }
                Ok(def)
            }
            _ => Err(parse_error(format!(
                "term definition for '{key}' must be a string or object"
            ))),
        }
    }

    fn context_for(&self, map: &serde_json::Map<String, Value>) -> Result<Context, OntolithError> {
        let mut ctx = self.context.clone();
        if let Some(c) = map.get("@context") {
            match c {
                Value::Object(_) | Value::Array(_) | Value::Null => {
                    let mut parser = JsonLdParser {
                        context: ctx.clone(),
                        dictionary: self.dictionary,
                        blank_count: self.blank_count,
                    };
                    parser.apply_context(c)?;
                    ctx = parser.context;
                }
                _ => {
                    return Err(OntolithError::Unsupported("json-ld remote @context URL"));
                }
            }
        }
        Ok(ctx)
    }

    /// Locate a keyword's value, honoring term aliases (`"id": "@id"`).
    fn node_keyword<'b>(
        &self,
        map: &'b serde_json::Map<String, Value>,
        keyword: &str,
        ctx: &Context,
    ) -> Option<&'b Value> {
        if let Some(v) = map.get(keyword) {
            return Some(v);
        }
        for (k, v) in map {
            if let Some(def) = ctx.terms.get(k)
                && def.id.as_deref() == Some(keyword)
            {
                return Some(v);
            }
        }
        None
    }

    fn mint_blank(&mut self) -> NodeId {
        let id = self
            .dictionary
            .encode_node(&format!("_:b{}", self.blank_count));
        self.blank_count += 1;
        id
    }

    fn iri_or_blank(&self, value: &str, ctx: &Context) -> Result<Term, OntolithError> {
        if value.starts_with("_:") {
            Ok(Term::blank(self.dictionary.encode_node(value)))
        } else {
            Ok(Term::iri(expand_iri_ref(ctx, value, false)))
        }
    }

    fn emit(
        &self,
        subject: NodeId,
        predicate: &str,
        object: Term,
        graph: Option<&Iri>,
        sink: &mut dyn RdfEventSink,
    ) -> Result<(), OntolithError> {
        let triple = Triple::new(subject, Iri::new(predicate), object);
        match graph {
            Some(g) => sink.on_event(RdfEvent::Quad(Quad::in_named_graph(triple, g.clone()))),
            None => sink.on_event(RdfEvent::Triple(triple)),
        }
    }

    fn process_graph(
        &mut self,
        value: &Value,
        graph: Option<Iri>,
        sink: &mut dyn RdfEventSink,
    ) -> Result<(), OntolithError> {
        match value {
            Value::Array(nodes) => {
                for node in nodes {
                    let _ = self.node_object(node, graph.clone(), sink)?;
                }
                Ok(())
            }
            other => {
                let _ = self.node_object(other, graph, sink)?;
                Ok(())
            }
        }
    }

    /// Expand a node object (or value object) and emit its triples. Returns
    /// the subject term for node objects; `None` for value objects.
    fn node_object(
        &mut self,
        value: &Value,
        graph: Option<Iri>,
        sink: &mut dyn RdfEventSink,
    ) -> Result<Option<Term>, OntolithError> {
        let map = value
            .as_object()
            .ok_or_else(|| parse_error("node object must be a JSON object"))?;
        if map.contains_key("@value") {
            return Ok(None);
        }
        let ctx = self.context_for(map)?;
        let subject_term = match self.node_keyword(map, "@id", &ctx).and_then(Value::as_str) {
            Some(id) => self.iri_or_blank(id, &ctx)?,
            None => Term::blank(self.mint_blank()),
        };
        let subject = match &subject_term {
            Term::Iri(iri) => self.dictionary.encode_node(iri.as_str()),
            Term::BlankNode(node) => *node,
            _ => unreachable!(),
        };

        if let Some(types) = self.node_keyword(map, "@type", &ctx) {
            let items = types
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![types.clone()]);
            for t in items {
                let t = t
                    .as_str()
                    .ok_or_else(|| parse_error("@type values must be strings"))?;
                let object = self.iri_or_blank(t, &ctx)?;
                self.emit(subject, RDF_TYPE, object, graph.as_ref(), sink)?;
            }
        }

        if let Some(graph_val) = self.node_keyword(map, "@graph", &ctx) {
            let named = match self.node_keyword(map, "@id", &ctx).and_then(Value::as_str) {
                Some(id) if !id.starts_with("_:") => {
                    Some(Iri::new(expand_iri_ref(&ctx, id, false)))
                }
                _ => None,
            };
            self.process_graph(graph_val, named.or(graph), sink)?;
            return Ok(Some(subject_term));
        }

        let node_language = self
            .node_keyword(map, "@language", &ctx)
            .and_then(Value::as_str);

        for (key, val) in map {
            if key.starts_with('@') {
                continue;
            }
            let def = ctx.terms.get(key).cloned().unwrap_or_default();
            let predicate = if let Some(id) = &def.id {
                id.clone()
            } else {
                expand_iri_ref(&ctx, key, true)
            };
            if predicate.starts_with('@') {
                continue;
            }
            self.emit_values(
                subject,
                &predicate,
                val,
                &def,
                node_language,
                graph.as_ref(),
                sink,
            )?;
        }
        Ok(Some(subject_term))
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_values(
        &mut self,
        subject: NodeId,
        predicate: &str,
        value: &Value,
        def: &TermDef,
        node_language: Option<&str>,
        graph: Option<&Iri>,
        sink: &mut dyn RdfEventSink,
    ) -> Result<(), OntolithError> {
        match def.container {
            Container::List => {
                let items = value
                    .as_array()
                    .ok_or_else(|| parse_error("@list container value must be an array"))?;
                self.emit_list(subject, predicate, items, def, node_language, graph, sink)?;
            }
            Container::Set => {
                for item in items_of(value) {
                    self.emit_value(subject, predicate, item, def, node_language, graph, sink)?;
                }
            }
            Container::Language => {
                let map = value
                    .as_object()
                    .ok_or_else(|| parse_error("@language container value must be an object"))?;
                for (lang, val) in map {
                    let tag = LanguageTag::parse(lang)
                        .map_err(|e| parse_error(format!("invalid language tag: {e}")))?;
                    for item in items_of(val) {
                        let obj = self.value_term(item, def, Some(tag.as_str()), sink)?;
                        if let Some(o) = obj {
                            self.emit(subject, predicate, o, graph, sink)?;
                        }
                    }
                }
            }
            Container::Index => {
                let map = value
                    .as_object()
                    .ok_or_else(|| parse_error("@index container value must be an object"))?;
                for val in map.values() {
                    for item in items_of(val) {
                        self.emit_value(subject, predicate, item, def, node_language, graph, sink)?;
                    }
                }
            }
            Container::None => {
                for item in items_of(value) {
                    self.emit_value(subject, predicate, item, def, node_language, graph, sink)?;
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_value(
        &mut self,
        subject: NodeId,
        predicate: &str,
        item: &Value,
        def: &TermDef,
        node_language: Option<&str>,
        graph: Option<&Iri>,
        sink: &mut dyn RdfEventSink,
    ) -> Result<(), OntolithError> {
        let obj = self.value_term(item, def, node_language, sink)?;
        if let Some(o) = obj {
            self.emit(subject, predicate, o, graph, sink)?;
        }
        Ok(())
    }

    fn value_term(
        &mut self,
        item: &Value,
        def: &TermDef,
        node_language: Option<&str>,
        sink: &mut dyn RdfEventSink,
    ) -> Result<Option<Term>, OntolithError> {
        match item {
            Value::Null => Ok(None),
            Value::String(s) => {
                if def.type_ == Some(TypeMapping::Id) {
                    Ok(Some(self.iri_or_blank(s, &self.context)?))
                } else if def.type_ == Some(TypeMapping::Vocab) {
                    Ok(Some(Term::iri(expand_iri_ref(&self.context, s, true))))
                } else if let Some(TypeMapping::Datatype(dt)) = &def.type_ {
                    Ok(Some(Term::literal(coerce_literal(s.clone(), dt))))
                } else if let Some(lang) = def
                    .language
                    .as_deref()
                    .or(node_language)
                    .or(self.context.default_language.as_deref())
                {
                    let tag = LanguageTag::parse(lang)
                        .map_err(|e| parse_error(format!("invalid language tag: {e}")))?;
                    Ok(Some(Term::literal(LiteralValue::Lang {
                        value: s.clone(),
                        lang: tag,
                    })))
                } else {
                    Ok(Some(Term::literal(LiteralValue::String(s.clone()))))
                }
            }
            Value::Number(n) => {
                let lv = if let Some(i) = n.as_i64() {
                    LiteralValue::Integer(i)
                } else {
                    LiteralValue::Double(n.as_f64().unwrap_or(0.0))
                };
                Ok(Some(Term::literal(lv)))
            }
            Value::Bool(b) => Ok(Some(Term::literal(LiteralValue::Boolean(*b)))),
            Value::Object(map) => {
                if map.contains_key("@value") {
                    self.value_object(map)
                } else if let Some(id) = map.get("@id").and_then(Value::as_str) {
                    let has_properties = map.keys().any(|k| !k.starts_with('@'));
                    if has_properties {
                        // Embedded node: emit nested triples and reference it.
                        self.node_object(item, None, sink)
                    } else {
                        // Pure reference to an existing node.
                        Ok(Some(self.iri_or_blank(id, &self.context)?))
                    }
                } else {
                    self.node_object(item, None, sink)
                }
            }
            _ => Err(parse_error("unsupported JSON value in property position")),
        }
    }

    fn value_object(
        &mut self,
        map: &serde_json::Map<String, Value>,
    ) -> Result<Option<Term>, OntolithError> {
        let v = map.get("@value").unwrap_or(&Value::Null);
        if v.is_null() {
            return Ok(None);
        }
        if let Some(t) = map.get("@type") {
            let dt = t
                .as_str()
                .ok_or_else(|| parse_error("@type in value object must be a string"))?;
            let dt = expand_iri_ref(&self.context, dt, true);
            let lexical = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => return Err(parse_error("invalid @value in value object")),
            };
            return Ok(Some(Term::literal(coerce_literal(lexical, &dt))));
        }
        if let Some(lang) = map.get("@language").and_then(Value::as_str) {
            let s = v
                .as_str()
                .ok_or_else(|| parse_error("@language requires a string @value"))?;
            let tag = LanguageTag::parse(lang)
                .map_err(|e| parse_error(format!("invalid language tag: {e}")))?;
            return Ok(Some(Term::literal(LiteralValue::Lang {
                value: s.to_owned(),
                lang: tag,
            })));
        }
        Ok(literal_from_value(v).map(Term::literal))
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_list(
        &mut self,
        subject: NodeId,
        predicate: &str,
        items: &[Value],
        def: &TermDef,
        node_language: Option<&str>,
        graph: Option<&Iri>,
        sink: &mut dyn RdfEventSink,
    ) -> Result<(), OntolithError> {
        if items.is_empty() {
            return self.emit(subject, predicate, Term::iri(RDF_NIL), graph, sink);
        }
        let head = self.mint_blank();
        self.emit(subject, predicate, Term::blank(head), graph, sink)?;
        let mut prev = head;
        let len = items.len();
        for (i, item) in items.iter().enumerate() {
            let obj = self.value_term(item, def, node_language, sink)?;
            if let Some(o) = obj {
                self.emit(prev, RDF_FIRST, o, graph, sink)?;
                if i + 1 < len {
                    let next = self.mint_blank();
                    self.emit(prev, RDF_REST, Term::blank(next), graph, sink)?;
                    prev = next;
                } else {
                    self.emit(prev, RDF_REST, Term::iri(RDF_NIL), graph, sink)?;
                }
            }
        }
        Ok(())
    }
}

fn items_of(value: &Value) -> Vec<&Value> {
    match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    }
}

/// Parse a JSON-LD document into the sink (streaming event contract).
pub fn parse_json_ld(
    input: &str,
    dictionary: &dyn DictionaryCodec,
    base_iri: Option<String>,
    sink: &mut dyn RdfEventSink,
) -> Result<(), OntolithError> {
    JsonLdParser::new(dictionary, base_iri).parse_into(input, sink)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::DatasetSink;
    use ontolith_storage::infrastructure::InMemoryDictionary;

    fn parse_doc(input: &str) -> (DatasetSink, InMemoryDictionary) {
        let dict = InMemoryDictionary::new();
        let mut sink = DatasetSink::default();
        parse_json_ld(input, &dict, None, &mut sink).expect("json-ld parse");
        (sink, dict)
    }

    fn assert_has(
        sink: &DatasetSink,
        dict: &InMemoryDictionary,
        subject: &str,
        predicate: &str,
        object: Term,
    ) {
        let expected = dict.encode_node(subject);
        let s = sink.dataset.default_graph.iter().any(|t| {
            t.subject == expected && t.predicate.as_str() == predicate && t.object == object
        });
        assert!(
            s,
            "missing triple {subject} {predicate} {object:?} in {:?}",
            sink.dataset.default_graph
        );
    }

    #[test]
    fn parses_compact_doc_with_context() {
        let (sink, dict) = parse_doc(
            r#"{
              "@context": {
                "name": "http://xmlns.com/foaf/0.1/name",
                "knows": {"@id": "http://xmlns.com/foaf/0.1/knows", "@type": "@id"}
              },
              "@id": "urn:alice",
              "name": "Alice",
              "knows": "urn:bob"
            }"#,
        );
        assert_eq!(sink.dataset.default_graph.len(), 2);
        assert_has(
            &sink,
            &dict,
            "urn:alice",
            "http://xmlns.com/foaf/0.1/name",
            Term::literal(LiteralValue::String("Alice".into())),
        );
        assert_has(
            &sink,
            &dict,
            "urn:alice",
            "http://xmlns.com/foaf/0.1/knows",
            Term::iri("urn:bob"),
        );
    }

    #[test]
    fn parses_language_and_typed_literals() {
        let (sink, dict) = parse_doc(
            r#"{
              "@context": {
                "xsd": "http://www.w3.org/2001/XMLSchema#",
                "label": "http://example.org/label",
                "age": {"@id": "http://example.org/age", "@type": "xsd:integer"},
                "typed": {"@id": "http://example.org/typed", "@type": "xsd:integer"}
              },
              "@id": "urn:p",
              "label": {"@value": "Hola", "@language": "es"},
              "age": 42,
              "typed": {"@value": "17", "@type": "xsd:integer"}
            }"#,
        );
        assert_has(
            &sink,
            &dict,
            "urn:p",
            "http://example.org/label",
            Term::literal(LiteralValue::Lang {
                value: "Hola".into(),
                lang: LanguageTag::parse("es").unwrap(),
            }),
        );
        assert_has(
            &sink,
            &dict,
            "urn:p",
            "http://example.org/age",
            Term::literal(LiteralValue::Integer(42)),
        );
        assert_has(
            &sink,
            &dict,
            "urn:p",
            "http://example.org/typed",
            Term::literal(LiteralValue::Integer(17)),
        );
    }

    #[test]
    fn parses_nested_nodes_and_lists() {
        let (sink, dict) = parse_doc(
            r#"{
              "@context": {
                "knows": {"@id": "http://xmlns.com/foaf/0.1/knows", "@type": "@id"},
                "name": "http://xmlns.com/foaf/0.1/name",
                "member": {"@id": "http://www.w3.org/1999/02/22-rdf-syntax-ns#member", "@type": "@id", "@container": "@list"}
              },
              "@id": "urn:alice",
              "knows": {"@id": "urn:bob", "name": "Bob"},
              "member": ["urn:x", "urn:y"]
            }"#,
        );
        assert_has(
            &sink,
            &dict,
            "urn:bob",
            "http://xmlns.com/foaf/0.1/name",
            Term::literal(LiteralValue::String("Bob".into())),
        );
        let list_head = sink.dataset.default_graph.iter().any(|t| {
            t.subject == dict.encode_node("urn:alice")
                && t.predicate.as_str() == "http://www.w3.org/1999/02/22-rdf-syntax-ns#member"
                && matches!(t.object, Term::BlankNode(_))
        });
        assert!(
            list_head,
            "urn:alice member must point to the list head blank node"
        );
        let firsts = sink
            .dataset
            .default_graph
            .iter()
            .filter(|t| t.predicate.as_str() == RDF_FIRST)
            .count();
        assert_eq!(firsts, 2);
        let rests = sink
            .dataset
            .default_graph
            .iter()
            .filter(|t| t.predicate.as_str() == RDF_REST)
            .count();
        assert_eq!(rests, 2);
    }

    #[test]
    fn parses_named_graph_quads() {
        let (sink, _) = parse_doc(
            r#"{
              "@context": {"name": "http://xmlns.com/foaf/0.1/name"},
              "@id": "urn:graph1",
              "@graph": [{"@id": "urn:a", "name": "A"}]
            }"#,
        );
        assert_eq!(sink.dataset.default_graph.len(), 0);
        assert_eq!(sink.dataset.named_graphs.len(), 1);
        assert_eq!(sink.dataset.named_graphs[0].name.as_str(), "urn:graph1");
        assert_eq!(sink.dataset.named_graphs[0].triples.len(), 1);
    }

    #[test]
    fn parses_prefix_expansion_and_vocab() {
        let (sink, dict) = parse_doc(
            r#"{
              "@context": {
                "ex": "http://example.org/",
                "@vocab": "http://schema.org/",
                "ex:name": "http://example.org/name"
              },
              "@id": "urn:a",
              "ex:name": "N",
              "name": "Schema"
            }"#,
        );
        assert_has(
            &sink,
            &dict,
            "urn:a",
            "http://example.org/name",
            Term::literal(LiteralValue::String("N".into())),
        );
        assert_has(
            &sink,
            &dict,
            "urn:a",
            "http://schema.org/name",
            Term::literal(LiteralValue::String("Schema".into())),
        );
    }

    #[test]
    fn parses_keyword_aliases() {
        let (sink, dict) = parse_doc(
            r#"{
              "@context": {
                "id": "@id",
                "type": "@type",
                "name": "http://xmlns.com/foaf/0.1/name"
              },
              "id": "urn:alice",
              "type": "urn:Person",
              "name": "Alice"
            }"#,
        );
        assert_has(&sink, &dict, "urn:alice", RDF_TYPE, Term::iri("urn:Person"));
        assert_has(
            &sink,
            &dict,
            "urn:alice",
            "http://xmlns.com/foaf/0.1/name",
            Term::literal(LiteralValue::String("Alice".into())),
        );
    }

    #[test]
    fn rejects_remote_context_and_bad_input() {
        let dict = InMemoryDictionary::new();
        let mut sink = DatasetSink::default();
        let err = parse_json_ld(
            r#"{"@context": "http://example.org/ctx", "@id": "urn:a"}"#,
            &dict,
            None,
            &mut sink,
        )
        .unwrap_err();
        assert!(matches!(err, OntolithError::Unsupported(_)));

        let err = parse_json_ld("{not json", &dict, None, &mut sink).unwrap_err();
        assert!(err.to_string().contains("invalid JSON"));
    }
}
