//! Production RDF/XML reader (RDF 1.1 XML syntax, pragmatic profile).
//!
//! Supports the constructions used by real-world ontology exports:
//! `rdf:RDF` roots, `rdf:Description` and typed-node elements,
//! `rdf:about` / `rdf:ID` / `rdf:nodeID`, property attributes, literal /
//! resource / nodeID property elements, `rdf:datatype` + `xml:lang`
//! literals, nested (implicit-blank) node property elements,
//! `rdf:parseType="Resource"`, `rdf:parseType="Collection"` with `rdf:li`
//! numbering, `xml:base`, entity / character references, and namespace
//! re-declarations scoped per element.
//!
//! Documented limitations (deterministic errors, matching the project's
//! profile-gated style): `rdf:parseType="Literal"` with XML markup content
//! and reification syntax are not supported.

use std::collections::BTreeMap;

use ontolith_core::domain::{Iri, LanguageTag, LiteralValue, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_storage::application::DictionaryCodec;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::term_lex::{coerce_typed_literal, resolve_against_base};
use crate::domain::{RdfEvent, RdfEventSink};

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// Parse an RDF/XML document into default-graph triple events.
pub(crate) fn parse_rdf_xml(
    input: &str,
    dictionary: &dyn DictionaryCodec,
    base_iri: Option<String>,
    sink: &mut dyn RdfEventSink,
) -> Result<(), OntolithError> {
    let root = build_dom(input)?;
    let doc = root.ok_or_else(|| OntolithError::failed("rdf/xml: empty document"))?;
    let mut out = Vec::new();
    let mut base = base_iri.unwrap_or_default();
    let mut seq = dictionary.len();
    let mut ctx = Ctx {
        dict: dictionary,
        base: &mut base,
        seq: &mut seq,
        out: &mut out,
    };

    if doc.ns.as_deref() == Some(RDF_NS) && doc.local == "RDF" {
        for child in &doc.children {
            if let Child::Elem(el) = child {
                process_node_element(el, "", &mut ctx)?;
            }
        }
    } else {
        process_node_element(&doc, "", &mut ctx)?;
    }
    for triple in out {
        sink.on_event(RdfEvent::Triple(triple))?;
    }
    Ok(())
}

/// Public convenience wrapper used by the parser surface and tests.
pub fn parse_rdf_xml_doc(
    input: &str,
    dictionary: &dyn DictionaryCodec,
    base_iri: Option<String>,
) -> Result<crate::domain::ParseOutput, OntolithError> {
    let mut sink = crate::domain::DatasetSink::default();
    parse_rdf_xml(input, dictionary, base_iri, &mut sink)?;
    let dataset = std::mem::take(&mut sink.dataset);
    Ok(crate::domain::ParseOutput {
        dataset,
        stats: sink.stats,
    })
}

// ---------------------------------------------------------------------------
// XML mini-DOM (namespace-aware)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Element {
    ns: Option<String>,
    local: String,
    attrs: Vec<Attr>,
    children: Vec<Child>,
}

#[derive(Debug)]
struct Attr {
    ns: Option<String>,
    local: String,
    value: String,
}

#[derive(Debug)]
enum Child {
    Elem(Element),
    Text(String),
}

fn build_dom(input: &str) -> Result<Option<Element>, OntolithError> {
    let mut reader = Reader::from_str(input);
    reader.config_mut().check_end_names = true;
    let mut buf: Vec<u8> = Vec::new();

    // Namespace frames: one map per open element plus the document frame.
    let mut ns_stack: Vec<BTreeMap<String, String>> = vec![BTreeMap::new()];
    ns_stack[0].insert("rdf".to_owned(), RDF_NS.to_owned());
    ns_stack[0].insert("xml".to_owned(), XML_NS.to_owned());

    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;

    let merge = |stack: &[BTreeMap<String, String>]| -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for frame in stack {
            for (k, v) in frame {
                out.insert(k.clone(), v.clone());
            }
        }
        out
    };

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let raw_name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let raw_attrs = read_raw_attrs(&e)?;
                let frame = declaration_frame(&raw_attrs);
                ns_stack.push(frame);
                let map = merge(&ns_stack);
                let (ns, local) = resolve_qname(&map, &raw_name);
                let attrs = raw_attrs
                    .iter()
                    .filter(|(k, _)| k != "xmlns" && !k.starts_with("xmlns:"))
                    .map(|(k, v)| {
                        let (ans, alocal) = resolve_qname(&map, k);
                        Attr {
                            ns: ans,
                            local: alocal,
                            value: v.clone(),
                        }
                    })
                    .collect();
                stack.push(Element {
                    ns,
                    local,
                    attrs,
                    children: Vec::new(),
                });
            }
            Ok(Event::Empty(e)) => {
                let raw_name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let raw_attrs = read_raw_attrs(&e)?;
                let frame = declaration_frame(&raw_attrs);
                ns_stack.push(frame);
                let map = merge(&ns_stack);
                let (ns, local) = resolve_qname(&map, &raw_name);
                let attrs = raw_attrs
                    .iter()
                    .filter(|(k, _)| k != "xmlns" && !k.starts_with("xmlns:"))
                    .map(|(k, v)| {
                        let (ans, alocal) = resolve_qname(&map, k);
                        Attr {
                            ns: ans,
                            local: alocal,
                            value: v.clone(),
                        }
                    })
                    .collect();
                ns_stack.pop();
                attach_element(
                    &mut stack,
                    &mut root,
                    Element {
                        ns,
                        local,
                        attrs,
                        children: Vec::new(),
                    },
                );
            }
            Ok(Event::Text(t)) => {
                if let Ok(decoded) = t.unescape() {
                    push_text(&mut stack, &decoded);
                }
            }
            Ok(Event::CData(t)) => {
                let text = String::from_utf8_lossy(t.as_ref()).into_owned();
                push_text(&mut stack, &text);
            }
            Ok(Event::End(_)) => {
                if let Some(closed) = stack.pop() {
                    attach_element(&mut stack, &mut root, closed);
                }
                if ns_stack.len() > 1 {
                    ns_stack.pop();
                }
            }
            Ok(Event::Eof) => break,
            Ok(Event::Decl(_))
            | Ok(Event::PI(_))
            | Ok(Event::Comment(_))
            | Ok(Event::DocType(_)) => {}
            Err(e) => return Err(OntolithError::failed(format!("rdf/xml: {e}"))),
        }
    }
    Ok(root)
}

fn read_raw_attrs(e: &BytesStart<'_>) -> Result<Vec<(String, String)>, OntolithError> {
    let mut out = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|err| OntolithError::failed(format!("rdf/xml attribute: {err}")))?;
        let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let value = a
            .unescape_value()
            .map_err(|err| OntolithError::failed(format!("rdf/xml attribute value: {err}")))?
            .into_owned();
        out.push((key, value));
    }
    Ok(out)
}

fn declaration_frame(raw_attrs: &[(String, String)]) -> BTreeMap<String, String> {
    let mut frame = BTreeMap::new();
    for (k, v) in raw_attrs {
        if let Some(prefix) = k.strip_prefix("xmlns:") {
            frame.insert(prefix.to_owned(), v.clone());
        } else if k == "xmlns" {
            frame.insert(String::new(), v.clone());
        }
    }
    frame
}

fn resolve_qname(map: &BTreeMap<String, String>, raw: &str) -> (Option<String>, String) {
    match raw.split_once(':') {
        Some((prefix, local)) => (map.get(prefix).cloned(), local.to_owned()),
        None => (map.get("").cloned(), raw.to_owned()),
    }
}

fn attach_element(stack: &mut [Element], root: &mut Option<Element>, el: Element) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(Child::Elem(el)),
        None => *root = Some(el),
    }
}

fn push_text(stack: &mut [Element], text: &str) {
    if let Some(top) = stack.last_mut() {
        top.children.push(Child::Text(text.to_owned()));
    }
}

// ---------------------------------------------------------------------------
// RDF/XML semantics
// ---------------------------------------------------------------------------

/// Mutable parse context threaded through the recursive node walk.
struct Ctx<'a> {
    dict: &'a dyn DictionaryCodec,
    base: &'a mut String,
    seq: &'a mut usize,
    out: &'a mut Vec<Triple>,
}

fn process_node_element(
    el: &Element,
    lang: &str,
    ctx: &mut Ctx<'_>,
) -> Result<NodeId, OntolithError> {
    let new_base = element_base(el, ctx.base);
    let inner_lang = element_lang(el, lang);
    *ctx.base = new_base;

    let about = attr(el, RDF_NS, "about");
    let id = attr(el, RDF_NS, "ID");
    let node_id = attr(el, RDF_NS, "nodeID");

    let subject = if let Some(a) = about {
        ctx.dict.encode_node(&resolve_ref(ctx.base, &a))
    } else if let Some(i) = id {
        ctx.dict.encode_node(&resolve_fragment(ctx.base, &i))
    } else if let Some(n) = node_id {
        ctx.dict.encode_node(&format!("_:{n}"))
    } else {
        fresh_blank(ctx.dict, ctx.seq)
    };

    let is_description = el.ns.as_deref() == Some(RDF_NS) && el.local == "Description";
    if !is_description {
        let type_iri = qname_iri(el).ok_or_else(|| {
            OntolithError::failed(format!(
                "rdf/xml: node element {} has no namespace",
                el.local
            ))
        })?;
        ctx.out.push(Triple::new(
            subject,
            Iri::new(RDF_TYPE),
            Term::Iri(Iri::new(type_iri)),
        ));
    }

    // Property attributes on the node element.
    for a in &el.attrs {
        if is_special_attr(a) {
            continue;
        }
        let Some(pred) = attr_iri(a) else { continue };
        let datatype = attr(el, RDF_NS, "datatype");
        let lit = build_literal(&a.value, datatype.as_deref(), &inner_lang);
        ctx.out
            .push(Triple::new(subject, Iri::new(pred), Term::Literal(lit)));
    }

    // Child property elements.
    let mut li_counter = 0usize;
    for child in &el.children {
        if let Child::Elem(prop) = child {
            process_property_element(subject, prop, &inner_lang, &mut li_counter, ctx)?;
        }
    }
    Ok(subject)
}

fn process_property_element(
    subject: NodeId,
    el: &Element,
    lang: &str,
    li_counter: &mut usize,
    ctx: &mut Ctx<'_>,
) -> Result<(), OntolithError> {
    let new_base = element_base(el, ctx.base);
    let inner_lang = element_lang(el, lang);
    *ctx.base = new_base;

    let predicate = if el.ns.as_deref() == Some(RDF_NS) && el.local == "li" {
        *li_counter += 1;
        format!("{RDF_NS}_{}", *li_counter)
    } else {
        qname_iri(el).ok_or_else(|| {
            OntolithError::failed(format!(
                "rdf/xml: property element {} has no namespace",
                el.local
            ))
        })?
    };
    let pred = Iri::new(&predicate);

    // rdf:resource / rdf:nodeID object shortcuts.
    if let Some(r) = attr(el, RDF_NS, "resource") {
        ctx.out.push(Triple::new(
            subject,
            pred,
            Term::Iri(Iri::new(resolve_ref(ctx.base, &r))),
        ));
        return Ok(());
    }
    if let Some(n) = attr(el, RDF_NS, "nodeID") {
        ctx.out.push(Triple::new(
            subject,
            pred,
            Term::BlankNode(ctx.dict.encode_node(&format!("_:{n}"))),
        ));
        return Ok(());
    }

    if let Some(pt) = attr(el, RDF_NS, "parseType") {
        match pt.as_str() {
            "Resource" => {
                let bnode = fresh_blank(ctx.dict, ctx.seq);
                ctx.out
                    .push(Triple::new(subject, pred, Term::BlankNode(bnode)));
                let mut nested_li = 0usize;
                for child in &el.children {
                    if let Child::Elem(c) = child {
                        process_property_element(bnode, c, &inner_lang, &mut nested_li, ctx)?;
                    }
                }
            }
            "Collection" => {
                let head = fresh_blank(ctx.dict, ctx.seq);
                ctx.out
                    .push(Triple::new(subject, pred, Term::BlankNode(head)));
                let items: Vec<&Element> = el
                    .children
                    .iter()
                    .filter_map(|c| match c {
                        Child::Elem(e) => Some(e),
                        Child::Text(_) => None,
                    })
                    .collect();
                let mut current = head;
                for (i, item) in items.iter().enumerate() {
                    let item_subject = process_node_element(item, &inner_lang, ctx)?;
                    ctx.out.push(Triple::new(
                        current,
                        Iri::new(format!("{RDF_NS}first")),
                        Term::BlankNode(item_subject),
                    ));
                    let rest = if i + 1 == items.len() {
                        ctx.dict.encode_node("_:rdf_nil")
                    } else {
                        fresh_blank(ctx.dict, ctx.seq)
                    };
                    ctx.out.push(Triple::new(
                        current,
                        Iri::new(format!("{RDF_NS}rest")),
                        Term::BlankNode(rest),
                    ));
                    current = rest;
                }
                if items.is_empty() {
                    let nil = ctx.dict.encode_node("_:rdf_nil");
                    ctx.out.push(Triple::new(
                        head,
                        Iri::new(format!("{RDF_NS}rest")),
                        Term::BlankNode(nil),
                    ));
                }
            }
            "Literal" => {
                if el.children.iter().any(|c| matches!(c, Child::Elem(_))) {
                    return Err(OntolithError::failed(
                        "rdf/xml: rdf:parseType=\"Literal\" with XML markup is not supported",
                    ));
                }
                let text = collect_text(el);
                ctx.out.push(Triple::new(
                    subject,
                    pred,
                    Term::Literal(LiteralValue::String(text)),
                ));
            }
            other => {
                return Err(OntolithError::failed(format!(
                    "rdf/xml: unknown rdf:parseType {other}"
                )));
            }
        }
        return Ok(());
    }

    // Element content: implicit blank node with nested property elements.
    let element_children: Vec<&Element> = el
        .children
        .iter()
        .filter_map(|c| match c {
            Child::Elem(e) => Some(e),
            Child::Text(_) => None,
        })
        .collect();
    if !element_children.is_empty() {
        let bnode = fresh_blank(ctx.dict, ctx.seq);
        ctx.out
            .push(Triple::new(subject, pred, Term::BlankNode(bnode)));
        let mut nested_li = 0usize;
        for child in element_children {
            process_property_element(bnode, child, &inner_lang, &mut nested_li, ctx)?;
        }
        return Ok(());
    }

    // Literal property.
    let datatype = attr(el, RDF_NS, "datatype");
    let text = collapse_whitespace(&collect_text(el));
    let lit = build_literal(&text, datatype.as_deref(), &inner_lang);
    ctx.out.push(Triple::new(subject, pred, Term::Literal(lit)));
    Ok(())
}

fn build_literal(value: &str, datatype: Option<&str>, lang: &str) -> LiteralValue {
    if let Some(dt) = datatype {
        return coerce_typed_literal(value.to_owned(), dt);
    }
    if !lang.is_empty() {
        return match LanguageTag::parse(lang) {
            Ok(tag) => LiteralValue::Lang {
                value: value.to_owned(),
                lang: tag,
            },
            Err(_) => LiteralValue::String(value.to_owned()),
        };
    }
    LiteralValue::String(value.to_owned())
}

fn qname_iri(el: &Element) -> Option<String> {
    let ns = el.ns.as_deref()?;
    Some(format!("{ns}{}", el.local))
}

fn attr_iri(a: &Attr) -> Option<String> {
    let ns = a.ns.as_deref()?;
    Some(format!("{ns}{}", a.local))
}

fn is_special_attr(a: &Attr) -> bool {
    if a.ns.as_deref() == Some(XML_NS) {
        return true;
    }
    if a.ns.as_deref() == Some(RDF_NS) {
        return matches!(
            a.local.as_str(),
            "about"
                | "ID"
                | "nodeID"
                | "resource"
                | "datatype"
                | "parseType"
                | "aboutEach"
                | "bagID"
        );
    }
    false
}

fn attr(el: &Element, ns: &str, local: &str) -> Option<String> {
    el.attrs
        .iter()
        .find(|a| a.ns.as_deref() == Some(ns) && a.local == local)
        .map(|a| a.value.clone())
}

fn element_base(el: &Element, current: &str) -> String {
    let raw = el
        .attrs
        .iter()
        .find(|a| a.ns.as_deref() == Some(XML_NS) && a.local == "base")
        .map(|a| a.value.clone());
    match raw {
        Some(b) => {
            if b.is_empty() {
                current.to_owned()
            } else {
                resolve_against_base(current, &b)
            }
        }
        None => current.to_owned(),
    }
}

fn element_lang(el: &Element, current: &str) -> String {
    el.attrs
        .iter()
        .find(|a| a.ns.as_deref() == Some(XML_NS) && a.local == "lang")
        .map(|a| a.value.clone())
        .unwrap_or_else(|| current.to_owned())
}

fn resolve_ref(base: &str, reference: &str) -> String {
    if reference.is_empty() {
        return base.to_owned();
    }
    if reference.contains(':') && !reference.starts_with("_:") {
        return reference.to_owned();
    }
    if base.is_empty() {
        return reference.to_owned();
    }
    resolve_against_base(base, reference)
}

fn resolve_fragment(base: &str, id: &str) -> String {
    if base.is_empty() {
        return format!("#{id}");
    }
    let stem = match base.find('#') {
        Some(pos) => &base[..pos],
        None => base,
    };
    format!("{stem}#{id}")
}

fn collect_text(el: &Element) -> String {
    let mut out = String::new();
    for child in &el.children {
        match child {
            Child::Text(t) => out.push_str(t),
            Child::Elem(_) => {}
        }
    }
    out
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn fresh_blank(dict: &dyn DictionaryCodec, seq: &mut usize) -> NodeId {
    loop {
        let label = format!("_:rdfxml{seq}");
        *seq += 1;
        if !dict.contains_value(&label) {
            return dict.encode_node(&label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::parse_rdf_xml_doc;
    use ontolith_storage::infrastructure::InMemoryDictionary;

    fn triples(doc: &str) -> (InMemoryDictionary, Vec<Triple>) {
        let dict = InMemoryDictionary::new();
        let parsed = parse_rdf_xml_doc(doc, &dict, Some("http://example.org/base/".into()))
            .expect("rdf/xml parse");
        (dict, parsed.dataset.default_graph.clone())
    }

    fn has<'a>(dict: &'a InMemoryDictionary, ts: &'a [Triple], s: &str, p: &str, o: &str) -> bool {
        let sid = dict.encode_node(s);
        ts.iter().any(|t| {
            t.subject == sid
                && t.predicate.as_str() == p
                && match &t.object {
                    Term::Iri(i) => i.as_str() == o,
                    Term::BlankNode(id) => dict.decode_node(*id).as_deref() == Some(o),
                    Term::Literal(l) => l.lexical_form() == o,
                }
        })
    }

    #[test]
    fn reads_description_resource_and_literal() {
        let xml = r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:ex="http://example.org/">
  <rdf:Description rdf:about="alice">
    <ex:knows rdf:resource="bob"/>
    <ex:name xml:lang="en">Alice &amp; Co</ex:name>
    <ex:age rdf:datatype="http://www.w3.org/2001/XMLSchema#integer">30</ex:age>
  </rdf:Description>
</rdf:RDF>"#;
        let (dict, ts) = triples(xml);
        assert_eq!(ts.len(), 3);
        assert!(has(
            &dict,
            &ts,
            "http://example.org/base/alice",
            "http://example.org/knows",
            "http://example.org/base/bob"
        ));
        assert!(has(
            &dict,
            &ts,
            "http://example.org/base/alice",
            "http://example.org/name",
            "Alice & Co"
        ));
        assert!(has(
            &dict,
            &ts,
            "http://example.org/base/alice",
            "http://example.org/age",
            "30"
        ));
    }

    #[test]
    fn reads_typed_node_and_property_attributes() {
        let xml = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
 xmlns:ex="http://example.org/">
 <ex:Person rdf:about="http://example.org/p1" ex:fullName="Alice"/>
</rdf:RDF>"#;
        let (dict, ts) = triples(xml);
        assert_eq!(ts.len(), 2);
        assert!(has(
            &dict,
            &ts,
            "http://example.org/p1",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
            "http://example.org/Person"
        ));
        assert!(has(
            &dict,
            &ts,
            "http://example.org/p1",
            "http://example.org/fullName",
            "Alice"
        ));
    }

    #[test]
    fn reads_node_id_and_parse_type_resource() {
        let xml = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
 xmlns:ex="http://example.org/">
 <rdf:Description rdf:about="http://example.org/a">
   <ex:parent rdf:parseType="Resource">
     <ex:name>Parent</ex:name>
   </ex:parent>
 </rdf:Description>
</rdf:RDF>"#;
        let (dict, ts) = triples(xml);
        assert_eq!(ts.len(), 2);
        assert!(has(
            &dict,
            &ts,
            "http://example.org/a",
            "http://example.org/parent",
            "_:rdfxml0"
        ));
    }

    #[test]
    fn reads_parse_type_collection_with_li() {
        let xml = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
 xmlns:ex="http://example.org/">
 <rdf:Description rdf:about="http://example.org/l">
   <ex:items rdf:parseType="Collection">
     <rdf:Description rdf:about="http://example.org/i1"/>
     <rdf:Description rdf:about="http://example.org/i2"/>
   </ex:items>
 </rdf:Description>
</rdf:RDF>"#;
        let (_dict, ts) = triples(xml);
        // head → first/rest chain over two items plus the container link.
        assert!(ts.len() >= 4);
        assert!(
            ts.iter()
                .any(|t| t.predicate.as_str() == "http://example.org/items")
        );
        assert!(ts
            .iter()
            .any(|t| t.predicate.as_str() == "http://www.w3.org/1999/02/22-rdf-syntax-ns#first"));
        assert!(
            ts.iter()
                .any(|t| t.predicate.as_str() == "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest")
        );
    }

    #[test]
    fn reads_rdf_id_and_nested_property_element() {
        let xml = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
 xmlns:ex="http://example.org/">
 <rdf:Description rdf:ID="item">
   <ex:child><ex:name>Nested</ex:name></ex:child>
 </rdf:Description>
</rdf:RDF>"#;
        let (dict, ts) = triples(xml);
        assert_eq!(ts.len(), 2);
        // rdf:ID resolves against the document base.
        let sid = dict.encode_node("http://example.org/base/#item");
        let child_triple = ts
            .iter()
            .find(|t| t.subject == sid && t.predicate.as_str() == "http://example.org/child")
            .expect("child property");
        // The nested property element creates an implicit blank object.
        let Term::BlankNode(blank) = child_triple.object else {
            panic!("nested object must be blank");
        };
        let label = dict.decode_node(blank).unwrap();
        assert!(label.starts_with("_:rdfxml"));
        assert!(ts.iter().any(|t| {
            t.subject == blank
                && t.predicate.as_str() == "http://example.org/name"
                && matches!(&t.object, Term::Literal(l) if l.lexical_form() == "Nested")
        }));
    }
}
