//! SHACL baseline validator (L6 — constraint subset).
//!
//! Supports node shapes and property shapes with the documented subset:
//! targets (`sh:targetClass` / `sh:targetNode` / `sh:targetSubjectsOf` /
//! `sh:targetObjectsOf` + implicit class target), constraints (`sh:class`,
//! `sh:datatype`, `sh:nodeKind`, `sh:minCount`/`sh:maxCount`,
//! `sh:minLength`/`sh:maxLength`, `sh:pattern`, `sh:in`, `sh:hasValue`,
//! `sh:node`, `sh:and`/`sh:or`/`sh:not`, `sh:qualifiedValueShape` with
//! `sh:qualifiedMinCount`/`sh:qualifiedMaxCount`/`sh:qualifiedValueShapesDisjoint`,
//! `sh:closed` with `sh:ignoredProperties`), severities and messages.
//! `sh:pattern` uses a small self-contained regex subset (literals, `.`,
//! `[...]`, `\d\w\s\D\W\S`, `*+?`, full-string match; no groups/alternation).

use crate::application::ShaclValidator;
use crate::domain::{
    ConstraintComponent, NodeKind, PropertyShape, SHACL_NS, Severity, Shape, Target,
    ValidationReport, ValidationResult, shacl,
};
use ontolith_core::domain::{Iri, LiteralValue, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_storage::application::DictionaryCodec;
use std::collections::{BTreeSet, HashMap};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
/// SHACL baseline validator.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShaclEngine;

impl ShaclEngine {
    pub fn new() -> Self {
        Self
    }
}

fn subject_key(dict: &dyn DictionaryCodec, node: NodeId) -> String {
    dict.decode_node(node)
        .unwrap_or_else(|| format!("_:n{}", node.get()))
}

fn term_key(dict: &dyn DictionaryCodec, term: &Term) -> String {
    match term {
        Term::Iri(iri) => iri.as_str().to_owned(),
        Term::BlankNode(id) => subject_key(dict, *id),
        Term::Literal(v) => format!("literal:{}|{}", v.lexical_form(), v.xsd_datatype_iri()),
    }
}

fn term_iri(term: &Term) -> Option<String> {
    match term {
        Term::Iri(iri) => Some(iri.as_str().to_owned()),
        _ => None,
    }
}

fn literal_usize(term: &Term) -> Option<usize> {
    match term {
        Term::Literal(LiteralValue::Integer(n)) if *n >= 0 => Some(*n as usize),
        _ => None,
    }
}

fn literal_string(term: &Term) -> Option<String> {
    match term {
        Term::Literal(LiteralValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn literal_bool(term: &Term) -> Option<bool> {
    match term {
        Term::Literal(LiteralValue::Boolean(b)) => Some(*b),
        _ => None,
    }
}

/// Index shapes-graph triples by subject key.
fn index_triples(
    dict: &dyn DictionaryCodec,
    triples: &[Triple],
) -> HashMap<String, Vec<(String, Term)>> {
    let mut map: HashMap<String, Vec<(String, Term)>> = HashMap::new();
    for t in triples {
        map.entry(subject_key(dict, t.subject))
            .or_default()
            .push((t.predicate.as_str().to_owned(), t.object.clone()));
    }
    map
}

fn is_shacl_body(triples: &[(String, Term)]) -> bool {
    triples.iter().any(|(p, _)| p.starts_with(SHACL_NS))
}

fn parse_severity(dict: &dyn DictionaryCodec, term: &Term) -> Option<Severity> {
    let key = term_key(dict, term);
    if key == shacl("Warning") {
        Some(Severity::Warning)
    } else if key == shacl("Info") {
        Some(Severity::Info)
    } else {
        Some(Severity::Violation)
    }
}

fn parse_node_kind(dict: &dyn DictionaryCodec, term: &Term) -> Option<NodeKind> {
    let key = term_key(dict, term);
    match key.as_str() {
        x if x == shacl("IRI") => Some(NodeKind::Iri),
        x if x == shacl("BlankNode") => Some(NodeKind::BlankNode),
        x if x == shacl("Literal") => Some(NodeKind::Literal),
        x if x == shacl("BlankNodeOrIRI") => Some(NodeKind::BlankNodeOrIri),
        x if x == shacl("IRIOrLiteral") => Some(NodeKind::IriOrLiteral),
        x if x == shacl("BlankNodeOrLiteral") => Some(NodeKind::BlankNodeOrLiteral),
        _ => None,
    }
}

/// Resolve an RDF collection starting at `start` into its members.
fn collect_list(
    dict: &dyn DictionaryCodec,
    map: &HashMap<String, Vec<(String, Term)>>,
    start: &Term,
) -> Vec<Term> {
    let mut out = Vec::new();
    let mut cur = start.clone();
    let mut seen = BTreeSet::new();
    loop {
        let key = term_key(dict, &cur);
        if !seen.insert(key.clone()) {
            break;
        }
        let Some(triples) = map.get(&key) else { break };
        let first = triples
            .iter()
            .find(|(p, _)| p == RDF_FIRST)
            .map(|(_, o)| o.clone());
        let rest = triples
            .iter()
            .find(|(p, _)| p == RDF_REST)
            .map(|(_, o)| o.clone());
        match (first, rest) {
            (Some(f), Some(r)) => {
                out.push(f);
                cur = r;
            }
            (Some(f), None) => {
                out.push(f);
                break;
            }
            _ => break,
        }
    }
    out
}

fn parse_property_shape(
    dict: &dyn DictionaryCodec,
    map: &HashMap<String, Vec<(String, Term)>>,
    key: &str,
) -> PropertyShape {
    let Some(triples) = map.get(key) else {
        return PropertyShape {
            path: String::new(),
            constraints: Vec::new(),
            severity: Severity::Violation,
            message: None,
        };
    };
    let path = triples
        .iter()
        .find(|(p, _)| p == &shacl("path"))
        .and_then(|(_, o)| term_iri(o))
        .unwrap_or_default();
    let (mut constraints, severity, message) = parse_shape_body(dict, map, triples);
    // Property-shape only parameters (must not appear on node shapes).
    for (p, o) in triples {
        match p.as_str() {
            x if x == shacl("qualifiedValueShape") => {
                constraints.push(ConstraintComponent::QualifiedValueShape {
                    shape: term_key(dict, o),
                });
            }
            x if x == shacl("qualifiedMinCount") => {
                if let Some(n) = literal_usize(o) {
                    constraints.push(ConstraintComponent::QualifiedMinCount(n));
                }
            }
            x if x == shacl("qualifiedMaxCount") => {
                if let Some(n) = literal_usize(o) {
                    constraints.push(ConstraintComponent::QualifiedMaxCount(n));
                }
            }
            x if x == shacl("qualifiedValueShapesDisjoint") && literal_bool(o) == Some(true) => {
                constraints.push(ConstraintComponent::QualifiedValueShapesDisjoint);
            }
            _ => {}
        }
    }
    PropertyShape {
        path,
        constraints,
        severity,
        message,
    }
}

/// Parse constraints / severity / message from one shape body.
/// Returns `(constraints, severity, message)`.
fn parse_shape_body(
    dict: &dyn DictionaryCodec,
    map: &HashMap<String, Vec<(String, Term)>>,
    triples: &[(String, Term)],
) -> (Vec<ConstraintComponent>, Severity, Option<String>) {
    let mut constraints = Vec::new();
    let mut severity = Severity::Violation;
    let mut message = None;
    for (p, o) in triples {
        match p.as_str() {
            x if x == shacl("class") => {
                if let Some(iri) = term_iri(o) {
                    constraints.push(ConstraintComponent::Class(iri));
                }
            }
            x if x == shacl("datatype") => {
                if let Some(iri) = term_iri(o) {
                    constraints.push(ConstraintComponent::Datatype(iri));
                }
            }
            x if x == shacl("nodeKind") => {
                if let Some(kind) = parse_node_kind(dict, o) {
                    constraints.push(ConstraintComponent::NodeKind(kind));
                }
            }
            x if x == shacl("minCount") => {
                if let Some(n) = literal_usize(o) {
                    constraints.push(ConstraintComponent::MinCount(n));
                }
            }
            x if x == shacl("maxCount") => {
                if let Some(n) = literal_usize(o) {
                    constraints.push(ConstraintComponent::MaxCount(n));
                }
            }
            x if x == shacl("minLength") => {
                if let Some(n) = literal_usize(o) {
                    constraints.push(ConstraintComponent::MinLength(n));
                }
            }
            x if x == shacl("maxLength") => {
                if let Some(n) = literal_usize(o) {
                    constraints.push(ConstraintComponent::MaxLength(n));
                }
            }
            x if x == shacl("pattern") => {
                if let Some(s) = literal_string(o) {
                    constraints.push(ConstraintComponent::Pattern(s));
                }
            }
            x if x == shacl("in") => {
                let values = collect_list(dict, map, o);
                if !values.is_empty() {
                    constraints.push(ConstraintComponent::In(values));
                }
            }
            x if x == shacl("hasValue") => {
                constraints.push(ConstraintComponent::HasValue(o.clone()));
            }
            x if x == shacl("node") => {
                constraints.push(ConstraintComponent::Node(term_key(dict, o)));
            }
            x if x == shacl("and") => {
                let shapes_list: Vec<String> = collect_list(dict, map, o)
                    .into_iter()
                    .map(|t| term_key(dict, &t))
                    .collect();
                if !shapes_list.is_empty() {
                    constraints.push(ConstraintComponent::And(shapes_list));
                }
            }
            x if x == shacl("or") => {
                let shapes_list: Vec<String> = collect_list(dict, map, o)
                    .into_iter()
                    .map(|t| term_key(dict, &t))
                    .collect();
                if !shapes_list.is_empty() {
                    constraints.push(ConstraintComponent::Or(shapes_list));
                }
            }
            x if x == shacl("not") => {
                constraints.push(ConstraintComponent::Not(term_key(dict, o)));
            }
            x if x == shacl("closed") => {
                if literal_bool(o) == Some(true) {
                    constraints.push(ConstraintComponent::Closed);
                }
            }
            x if x == shacl("severity") => {
                if let Some(s) = parse_severity(dict, o) {
                    severity = s;
                }
            }
            x if x == shacl("message") => {
                if let Some(s) = literal_string(o) {
                    message = Some(s);
                }
            }
            _ => {}
        }
    }
    (constraints, severity, message)
}

fn parse_shapes(dict: &dyn DictionaryCodec, shapes: &[Triple]) -> Vec<Shape> {
    let map = index_triples(dict, shapes);
    let mut out = Vec::new();
    for (id, triples) in &map {
        if !is_shacl_body(triples) {
            continue;
        }
        let mut targets = Vec::new();
        let mut property_shapes = Vec::new();
        let mut ignored_properties = Vec::new();
        let (constraints, severity, message) = parse_shape_body(dict, &map, triples);
        for (p, o) in triples {
            match p.as_str() {
                x if x == shacl("targetClass") => {
                    if let Some(iri) = term_iri(o) {
                        targets.push(Target::Class(iri));
                    }
                }
                x if x == shacl("targetNode") => targets.push(Target::Node(o.clone())),
                x if x == shacl("targetSubjectsOf") => {
                    if let Some(iri) = term_iri(o) {
                        targets.push(Target::SubjectsOf(iri));
                    }
                }
                x if x == shacl("targetObjectsOf") => {
                    if let Some(iri) = term_iri(o) {
                        targets.push(Target::ObjectsOf(iri));
                    }
                }
                x if x == shacl("property") => {
                    let key = term_key(dict, o);
                    property_shapes.push(parse_property_shape(dict, &map, &key));
                }
                x if x == shacl("ignoredProperties") => {
                    ignored_properties = collect_list(dict, &map, o)
                        .into_iter()
                        .filter_map(|t| term_iri(&t))
                        .collect();
                }
                _ => {}
            }
        }
        out.push(Shape {
            id: id.clone(),
            targets,
            constraints,
            property_shapes,
            ignored_properties,
            severity,
            message,
        });
    }
    out
}

fn select_targets(dict: &dyn DictionaryCodec, data: &[Triple], shape: &Shape) -> Vec<String> {
    let mut targets = BTreeSet::new();
    for target in &shape.targets {
        match target {
            Target::Node(n) => {
                targets.insert(term_key(dict, n));
            }
            Target::Class(class) => {
                for t in data {
                    if t.predicate.as_str() == RDF_TYPE && term_key(dict, &t.object) == *class {
                        targets.insert(subject_key(dict, t.subject));
                    }
                }
            }
            Target::SubjectsOf(p) => {
                for t in data {
                    if t.predicate.as_str() == p {
                        targets.insert(subject_key(dict, t.subject));
                    }
                }
            }
            Target::ObjectsOf(p) => {
                for t in data {
                    if t.predicate.as_str() == p {
                        targets.insert(term_key(dict, &t.object));
                    }
                }
            }
        }
    }
    // Implicit class target: sh:class also targets instances of that class.
    for class in shape.constraints.iter().filter_map(|c| match c {
        ConstraintComponent::Class(cl) => Some(cl),
        _ => None,
    }) {
        for t in data {
            if t.predicate.as_str() == RDF_TYPE && term_key(dict, &t.object) == *class {
                targets.insert(subject_key(dict, t.subject));
            }
        }
    }
    targets.into_iter().collect()
}

fn is_instance_of(dict: &dyn DictionaryCodec, data: &[Triple], node: &str, class: &str) -> bool {
    data.iter().any(|t| {
        subject_key(dict, t.subject) == node
            && t.predicate.as_str() == RDF_TYPE
            && term_key(dict, &t.object) == class
    })
}

fn default_message(component: &str, detail: &str) -> String {
    match component {
        c if c == shacl("ClassConstraintComponent") => {
            format!("value is not an instance of {detail}")
        }
        c if c == shacl("DatatypeConstraintComponent") => {
            format!("value is not a literal with datatype {detail}")
        }
        c if c == shacl("NodeKindConstraintComponent") => {
            format!("value does not have node kind {detail}")
        }
        c if c == shacl("MinCountConstraintComponent") => {
            format!("fewer than {detail} values")
        }
        c if c == shacl("MaxCountConstraintComponent") => {
            format!("more than {detail} values")
        }
        c if c == shacl("MinLengthConstraintComponent") => {
            format!("string shorter than {detail} characters")
        }
        c if c == shacl("MaxLengthConstraintComponent") => {
            format!("string longer than {detail} characters")
        }
        c if c == shacl("PatternConstraintComponent") => {
            format!("string does not match pattern /{detail}/")
        }
        c if c == shacl("InConstraintComponent") => "value not in allowed list".to_owned(),
        c if c == shacl("HasValueConstraintComponent") => "required value not present".to_owned(),
        c if c == shacl("NodeConstraintComponent") => "node does not conform".to_owned(),
        c if c == shacl("AndConstraintComponent") => {
            "value does not conform to all shapes in sh:and".to_owned()
        }
        c if c == shacl("OrConstraintComponent") => {
            "value does not conform to any shape in sh:or".to_owned()
        }
        c if c == shacl("NotConstraintComponent") => {
            "value conforms to the shape in sh:not".to_owned()
        }
        c if c == shacl("QualifiedMinCountConstraintComponent") => {
            format!("fewer than {detail} values conform to the qualified value shape")
        }
        c if c == shacl("QualifiedMaxCountConstraintComponent") => {
            format!("more than {detail} values conform to the qualified value shape")
        }
        c if c == shacl("ClosedConstraintComponent") => {
            "property not allowed by closed shape".to_owned()
        }
        _ => "constraint violated".to_owned(),
    }
}

#[allow(clippy::too_many_arguments)]
fn push_result(
    results: &mut Vec<ValidationResult>,
    focus: &str,
    path: Option<String>,
    value: Option<String>,
    source_shape: Option<&str>,
    component: &str,
    detail: &str,
    severity: Severity,
    message: Option<&str>,
) {
    results.push(ValidationResult {
        focus_node: focus.to_owned(),
        path,
        value,
        source_shape: source_shape.map(str::to_owned),
        component: component.to_owned(),
        severity,
        message: Some(
            message
                .map(str::to_owned)
                .unwrap_or_else(|| default_message(component, detail)),
        ),
    });
}

#[allow(clippy::too_many_arguments)]
fn check_values(
    dict: &dyn DictionaryCodec,
    data: &[Triple],
    shapes: &[Shape],
    focus: &str,
    path: Option<&str>,
    values: &[(String, Term)],
    constraint: &ConstraintComponent,
    source_shape: Option<&str>,
    ps: Option<&PropertyShape>,
    severity: Severity,
    message: Option<&str>,
    results: &mut Vec<ValidationResult>,
    depth: usize,
) {
    match constraint {
        ConstraintComponent::Class(class) => {
            for (vkey, _) in values {
                if !is_instance_of(dict, data, vkey, class) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("ClassConstraintComponent"),
                        class,
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::Datatype(dt) => {
            for (vkey, v) in values {
                let ok = matches!(v, Term::Literal(lv) if lv.xsd_datatype_iri().as_str() == dt);
                if !ok {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("DatatypeConstraintComponent"),
                        dt,
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::NodeKind(kind) => {
            for (vkey, v) in values {
                let ok = matches!(
                    (kind, v),
                    (NodeKind::Iri, Term::Iri(_))
                        | (NodeKind::BlankNode, Term::BlankNode(_))
                        | (NodeKind::Literal, Term::Literal(_))
                        | (NodeKind::BlankNodeOrIri, Term::BlankNode(_) | Term::Iri(_))
                        | (NodeKind::IriOrLiteral, Term::Iri(_) | Term::Literal(_))
                        | (
                            NodeKind::BlankNodeOrLiteral,
                            Term::BlankNode(_) | Term::Literal(_)
                        )
                );
                if !ok {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("NodeKindConstraintComponent"),
                        kind_name(kind),
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::MinLength(n) => {
            for (vkey, v) in values {
                let len = match v {
                    Term::Literal(lv) => Some(lv.lexical_form().chars().count()),
                    _ => None,
                };
                if !len.is_some_and(|l| l >= *n) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("MinLengthConstraintComponent"),
                        &n.to_string(),
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::MaxLength(n) => {
            for (vkey, v) in values {
                let len = match v {
                    Term::Literal(lv) => Some(lv.lexical_form().chars().count()),
                    _ => None,
                };
                if !len.is_some_and(|l| l <= *n) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("MaxLengthConstraintComponent"),
                        &n.to_string(),
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::Pattern(pat) => {
            for (vkey, v) in values {
                let ok = match v {
                    Term::Literal(lv) => Some(pattern_matches(pat, &lv.lexical_form())),
                    _ => None,
                };
                if ok != Some(true) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("PatternConstraintComponent"),
                        pat,
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::In(allowed) => {
            let allowed_keys: Vec<String> = allowed.iter().map(|t| term_key(dict, t)).collect();
            for (vkey, _) in values {
                if !allowed_keys.contains(vkey) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("InConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::HasValue(required) => {
            let wanted = term_key(dict, required);
            let found = values.iter().any(|(k, _)| *k == wanted);
            if !found {
                push_result(
                    results,
                    focus,
                    path.map(str::to_owned),
                    Some(wanted),
                    source_shape,
                    &shacl("HasValueConstraintComponent"),
                    "",
                    severity,
                    message,
                );
            }
        }
        ConstraintComponent::Node(shape_key) => {
            for (vkey, _) in values {
                evaluate_shape(dict, data, shapes, shape_key, vkey, results, depth + 1);
            }
        }
        ConstraintComponent::And(shape_keys) => {
            for (vkey, _) in values {
                let conforms = shape_keys
                    .iter()
                    .all(|k| conforms_to(dict, data, shapes, k, vkey, depth));
                if !conforms {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("AndConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::Or(shape_keys) => {
            for (vkey, _) in values {
                let conforms = shape_keys
                    .iter()
                    .any(|k| conforms_to(dict, data, shapes, k, vkey, depth));
                if !conforms {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("OrConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::Not(shape_key) => {
            for (vkey, _) in values {
                if conforms_to(dict, data, shapes, shape_key, vkey, depth) {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        Some(vkey.clone()),
                        source_shape,
                        &shacl("NotConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::QualifiedValueShape { .. }
        | ConstraintComponent::QualifiedValueShapesDisjoint => {}
        ConstraintComponent::QualifiedMinCount(n) => {
            if let Some((shape_key, disjoint, siblings)) =
                qualified_value_context(shapes, source_shape, ps)
            {
                let count = values
                    .iter()
                    .filter(|(vkey, _)| {
                        let mut matches = conforms_to(dict, data, shapes, &shape_key, vkey, depth);
                        if matches && disjoint {
                            matches = !siblings
                                .iter()
                                .any(|s| conforms_to(dict, data, shapes, s, vkey, depth));
                        }
                        matches
                    })
                    .count();
                if count < *n {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        None,
                        source_shape,
                        &shacl("QualifiedMinCountConstraintComponent"),
                        &n.to_string(),
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::QualifiedMaxCount(n) => {
            if let Some((shape_key, disjoint, siblings)) =
                qualified_value_context(shapes, source_shape, ps)
            {
                let count = values
                    .iter()
                    .filter(|(vkey, _)| {
                        let mut matches = conforms_to(dict, data, shapes, &shape_key, vkey, depth);
                        if matches && disjoint {
                            matches = !siblings
                                .iter()
                                .any(|s| conforms_to(dict, data, shapes, s, vkey, depth));
                        }
                        matches
                    })
                    .count();
                if count > *n {
                    push_result(
                        results,
                        focus,
                        path.map(str::to_owned),
                        None,
                        source_shape,
                        &shacl("QualifiedMaxCountConstraintComponent"),
                        &n.to_string(),
                        severity,
                        message,
                    );
                }
            }
        }
        ConstraintComponent::MinCount(n) => {
            if values.len() < *n {
                push_result(
                    results,
                    focus,
                    path.map(str::to_owned),
                    None,
                    source_shape,
                    &shacl("MinCountConstraintComponent"),
                    &n.to_string(),
                    severity,
                    message,
                );
            }
        }
        ConstraintComponent::MaxCount(n) => {
            if values.len() > *n {
                push_result(
                    results,
                    focus,
                    path.map(str::to_owned),
                    None,
                    source_shape,
                    &shacl("MaxCountConstraintComponent"),
                    &n.to_string(),
                    severity,
                    message,
                );
            }
        }
        ConstraintComponent::Closed => {
            let allowed: Vec<String> = shapes
                .iter()
                .find(|s| s.id == *source_shape.unwrap_or(""))
                .map(|s| {
                    let mut paths: Vec<String> = s
                        .property_shapes
                        .iter()
                        .map(|ps| ps.path.clone())
                        .collect();
                    paths.extend(s.ignored_properties.iter().cloned());
                    paths
                })
                .unwrap_or_default();
            for t in data {
                if subject_key(dict, t.subject) == focus
                    && !allowed.iter().any(|p| p == t.predicate.as_str())
                    && t.predicate.as_str() != RDF_TYPE
                {
                    push_result(
                        results,
                        focus,
                        Some(t.predicate.as_str().to_owned()),
                        Some(term_key(dict, &t.object)),
                        source_shape,
                        &shacl("ClosedConstraintComponent"),
                        "",
                        severity,
                        message,
                    );
                }
            }
        }
    }
}

fn kind_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Iri => "IRI",
        NodeKind::BlankNode => "BlankNode",
        NodeKind::Literal => "Literal",
        NodeKind::BlankNodeOrIri => "BlankNodeOrIRI",
        NodeKind::IriOrLiteral => "IRIOrLiteral",
        NodeKind::BlankNodeOrLiteral => "BlankNodeOrLiteral",
    }
}

/// Look up the `sh:qualifiedValueShape` parameter of the current property shape.
/// Returns `(shape_key, sh:qualifiedValueShapesDisjoint, sibling_qualified_shapes)`.
fn qualified_value_context<'a>(
    shapes: &'a [Shape],
    source_shape: Option<&str>,
    ps: Option<&'a PropertyShape>,
) -> Option<(String, bool, Vec<&'a str>)> {
    let shape = shapes
        .iter()
        .find(|s| s.id == source_shape.unwrap_or(""))?;
    let ps = ps?;
    let shape_key = ps.constraints.iter().find_map(|c| match c {
        ConstraintComponent::QualifiedValueShape { shape } => Some(shape.clone()),
        _ => None,
    })?;
    let disjoint = ps
        .constraints
        .iter()
        .any(|c| matches!(c, ConstraintComponent::QualifiedValueShapesDisjoint));
    let siblings: Vec<&str> = shape
        .property_shapes
        .iter()
        .filter(|other| !std::ptr::eq(*other, ps))
        .filter_map(|other| {
            other.constraints.iter().find_map(|c| match c {
                ConstraintComponent::QualifiedValueShape { shape } => Some(shape.as_str()),
                _ => None,
            })
        })
        .collect();
    Some((shape_key, disjoint, siblings))
}

/// True if evaluating `shape_key` with focus node `focus` produces no `sh:Violation` results.
fn conforms_to(
    dict: &dyn DictionaryCodec,
    data: &[Triple],
    shapes: &[Shape],
    shape_key: &str,
    focus: &str,
    depth: usize,
) -> bool {
    if depth > 32 {
        return true;
    }
    let mut local = Vec::new();
    evaluate_shape(dict, data, shapes, shape_key, focus, &mut local, depth + 1);
    local.iter().all(|r| r.severity != Severity::Violation)
}

fn evaluate_shape(
    dict: &dyn DictionaryCodec,
    data: &[Triple],
    shapes: &[Shape],
    shape_key: &str,
    focus: &str,
    results: &mut Vec<ValidationResult>,
    depth: usize,
) {
    if depth > 32 {
        return;
    }
    let Some(shape) = shapes.iter().find(|s| s.id == shape_key) else {
        return;
    };
    // Node-shape constraints apply to the focus node itself.
    let focus_values = vec![(focus.to_owned(), focus_as_term(focus))];
    for c in &shape.constraints {
        check_values(
            dict,
            data,
            shapes,
            focus,
            None,
            &focus_values,
            c,
            Some(&shape.id),
            None,
            shape.severity,
            shape.message.as_deref(),
            results,
            depth,
        );
    }
    // Property shapes apply to values of the path.
    for ps in &shape.property_shapes {
        let values: Vec<(String, Term)> = data
            .iter()
            .filter(|t| subject_key(dict, t.subject) == focus && t.predicate.as_str() == ps.path)
            .map(|t| (term_key(dict, &t.object), t.object.clone()))
            .collect();
        for c in &ps.constraints {
            check_values(
                dict,
                data,
                shapes,
                focus,
                Some(&ps.path),
                &values,
                c,
                Some(&shape.id),
                Some(ps),
                ps.severity,
                ps.message.as_deref(),
                results,
                depth,
            );
        }
    }
}

/// Materialize a focus node key back to a `Term` for constraint evaluation.
fn focus_as_term(key: &str) -> Term {
    if let Some(rest) = key.strip_prefix("literal:") {
        let (lex, dt) = rest.rsplit_once('|').unwrap_or((rest, ""));
        if dt == "http://www.w3.org/2001/XMLSchema#integer"
            && let Ok(n) = lex.parse::<i64>()
        {
            return Term::Literal(LiteralValue::Integer(n));
        }
        if dt == "http://www.w3.org/2001/XMLSchema#boolean"
            && let Ok(b) = lex.parse::<bool>()
        {
            return Term::Literal(LiteralValue::Boolean(b));
        }
        if dt == "http://www.w3.org/2001/XMLSchema#double"
            && let Ok(f) = lex.parse::<f64>()
        {
            return Term::Literal(LiteralValue::Decimal(f));
        }
        return Term::Literal(LiteralValue::String(lex.to_owned()));
    }
    Term::Iri(Iri::new(key))
}

impl ShaclValidator for ShaclEngine {
    fn validate(
        &self,
        dict: &dyn DictionaryCodec,
        shapes: &[Triple],
        data: &[Triple],
    ) -> Result<ValidationReport, OntolithError> {
        let shapes = parse_shapes(dict, shapes);
        let mut results = Vec::new();
        for shape in &shapes {
            for focus in select_targets(dict, data, shape) {
                evaluate_shape(dict, data, &shapes, &shape.id, &focus, &mut results, 0);
            }
        }
        results.sort_by(|a, b| {
            a.focus_node
                .cmp(&b.focus_node)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.value.cmp(&b.value))
        });
        let conforms = results.iter().all(|r| r.severity != Severity::Violation);
        Ok(ValidationReport { conforms, results })
    }
}

// ---------------------------------------------------------------------------
// Minimal regex subset (full-string match; no groups/alternation)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Atom {
    Literal(char),
    Any,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    Digit,
    Word,
    Space,
    NonDigit,
    NonWord,
    NonSpace,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Quant {
    Star,
    Plus,
    Question,
}

fn parse_atom(chars: &[char], i: &mut usize) -> Option<Atom> {
    let c = chars[*i];
    *i += 1;
    match c {
        '.' => Some(Atom::Any),
        '\\' => {
            let e = *chars.get(*i)?;
            *i += 1;
            Some(match e {
                'd' => Atom::Digit,
                'w' => Atom::Word,
                's' => Atom::Space,
                'D' => Atom::NonDigit,
                'W' => Atom::NonWord,
                'S' => Atom::NonSpace,
                other => Atom::Literal(other),
            })
        }
        '[' => {
            let mut negated = false;
            if chars.get(*i) == Some(&'^') {
                negated = true;
                *i += 1;
            }
            let mut ranges = Vec::new();
            loop {
                let lo = *chars.get(*i)?;
                *i += 1;
                if lo == ']' {
                    break;
                }
                if lo == '\\' {
                    let e = *chars.get(*i)?;
                    *i += 1;
                    let hi = if chars.get(*i) == Some(&'-')
                        && chars.get(*i + 1).is_some_and(|n| *n != ']')
                    {
                        *i += 1;
                        let h = *chars.get(*i)?;
                        *i += 1;
                        Some(h)
                    } else {
                        None
                    };
                    ranges.push((e, hi.unwrap_or(e)));
                    continue;
                }
                if chars.get(*i) == Some(&'-') && chars.get(*i + 1).is_some_and(|n| *n != ']') {
                    let hi = *chars.get(*i + 1)?;
                    *i += 2;
                    ranges.push((lo, hi));
                } else {
                    ranges.push((lo, lo));
                }
            }
            Some(Atom::Class { negated, ranges })
        }
        other => Some(Atom::Literal(other)),
    }
}

fn parse_pattern(pattern: &str) -> Vec<(Atom, Option<Quant>)> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    let mut tokens = Vec::new();
    while i < chars.len() {
        if chars[i] == '^' {
            i += 1;
            continue;
        }
        if chars[i] == '$' {
            i += 1;
            continue;
        }
        let atom = parse_atom(&chars, &mut i).unwrap_or(Atom::Literal(chars[i - 1]));
        let quant = match chars.get(i) {
            Some('*') => {
                i += 1;
                Some(Quant::Star)
            }
            Some('+') => {
                i += 1;
                Some(Quant::Plus)
            }
            Some('?') => {
                i += 1;
                Some(Quant::Question)
            }
            _ => None,
        };
        tokens.push((atom, quant));
    }
    tokens
}

fn atom_matches(atom: &Atom, c: char) -> bool {
    match atom {
        Atom::Literal(l) => *l == c,
        Atom::Any => true,
        Atom::Digit => c.is_ascii_digit(),
        Atom::Word => c.is_ascii_alphanumeric() || c == '_',
        Atom::Space => c.is_ascii_whitespace(),
        Atom::NonDigit => !c.is_ascii_digit(),
        Atom::NonWord => !(c.is_ascii_alphanumeric() || c == '_'),
        Atom::NonSpace => !c.is_ascii_whitespace(),
        Atom::Class { negated, ranges } => {
            let hit = ranges.iter().any(|(lo, hi)| *lo <= c && c <= *hi);
            hit != *negated
        }
    }
}

fn match_at(tokens: &[(Atom, Option<Quant>)], ti: usize, text: &[char], ci: usize) -> bool {
    if ti == tokens.len() {
        return ci == text.len();
    }
    let (atom, quant) = &tokens[ti];
    match quant {
        None => {
            if ci < text.len() && atom_matches(atom, text[ci]) {
                match_at(tokens, ti + 1, text, ci + 1)
            } else {
                false
            }
        }
        Some(Quant::Question) => {
            match_at(tokens, ti + 1, text, ci)
                || (ci < text.len()
                    && atom_matches(atom, text[ci])
                    && match_at(tokens, ti + 1, text, ci + 1))
        }
        Some(Quant::Star) => {
            match_at(tokens, ti + 1, text, ci)
                || (ci < text.len()
                    && atom_matches(atom, text[ci])
                    && match_at(tokens, ti, text, ci + 1))
        }
        Some(Quant::Plus) => {
            ci < text.len()
                && atom_matches(atom, text[ci])
                && (match_at(tokens, ti + 1, text, ci + 1) || match_at(tokens, ti, text, ci + 1))
        }
    }
}

fn pattern_matches(pattern: &str, text: &str) -> bool {
    let tokens = parse_pattern(pattern);
    let text: Vec<char> = text.chars().collect();
    match_at(&tokens, 0, &text, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_parser::infrastructure::parse_turtle_doc;
    use ontolith_storage::infrastructure::InMemoryDictionary;

    fn load_turtle(dict: &InMemoryDictionary, src: &str) -> Vec<Triple> {
        parse_turtle_doc(src, dict)
            .expect("turtle parse")
            .dataset
            .default_graph
    }

    fn validate(dict: &InMemoryDictionary, shapes_src: &str, data_src: &str) -> ValidationReport {
        let shapes = load_turtle(dict, shapes_src);
        let data = load_turtle(dict, data_src);
        ShaclEngine::new()
            .validate(dict, &shapes, &data)
            .expect("validation")
    }

    const SH: &str = "PREFIX sh: <http://www.w3.org/ns/shacl#> PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> PREFIX ex: <http://ex.org/> ";

    #[test]
    fn class_constraint_violation() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ; sh:class ex:Adult ."
            ),
            &format!(
                "{SH}
                ex:alice a ex:Person, ex:Adult . ex:bob a ex:Person ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/bob");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#ClassConstraintComponent"
        );
    }

    #[test]
    fn conforming_data_passes() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ; sh:class ex:Adult ."
            ),
            &format!(
                "{SH}
                ex:alice a ex:Person, ex:Adult . ex:bob a ex:Person, ex:Adult ."
            ),
        );
        assert!(report.conforms);
        assert!(report.results.is_empty());
    }

    #[test]
    fn property_min_count_and_datatype() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:OrderShape a sh:NodeShape ; sh:targetClass ex:Order ;
                    sh:property [ sh:path ex:status ; sh:minCount 1 ; sh:datatype xsd:string ;
                                  sh:message \"status required as string\" ] ."
            ),
            &format!(
                "{SH}
                ex:o1 a ex:Order ; ex:status \"open\" . ex:o2 a ex:Order ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/o2");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#MinCountConstraintComponent"
        );
        assert_eq!(
            report.results[0].message.as_deref(),
            Some("status required as string")
        );
    }

    #[test]
    fn pattern_and_min_length() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:UserShape a sh:NodeShape ; sh:targetClass ex:User ;
                    sh:property [ sh:path ex:email ; sh:nodeKind sh:Literal ;
                                  sh:pattern \"^[a-z]+@[a-z]+\\\\.[a-z]+$\" ; sh:minLength 5 ] ."
            ),
            &format!(
                "{SH}
                ex:u1 a ex:User ; ex:email \"alice@example.com\" .
                ex:u2 a ex:User ; ex:email \"BAD\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        assert!(
            report
                .results
                .iter()
                .all(|r| r.focus_node == "http://ex.org/u2")
        );
        let components: Vec<_> = report
            .results
            .iter()
            .map(|r| r.component.as_str())
            .collect();
        assert!(components.contains(&"http://www.w3.org/ns/shacl#PatternConstraintComponent"));
        assert!(components.contains(&"http://www.w3.org/ns/shacl#MinLengthConstraintComponent"));
    }

    #[test]
    fn closed_shape_rejects_extra_property() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:StrictShape a sh:NodeShape ; sh:targetClass ex:Strict ; sh:closed true ;
                    sh:property [ sh:path ex:name ; sh:minCount 1 ] ."
            ),
            &format!(
                "{SH}
                ex:s1 a ex:Strict ; ex:name \"ok\" .
                ex:s2 a ex:Strict ; ex:name \"x\" ; ex:extra 1 ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(
            report.results[0].path.as_deref(),
            Some("http://ex.org/extra")
        );
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#ClosedConstraintComponent"
        );
    }

    #[test]
    fn in_constraint_rejects_unknown_value() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TaskShape a sh:NodeShape ; sh:targetClass ex:Task ;
                    sh:property [ sh:path ex:status ; sh:in ( \"open\" \"closed\" ) ] ."
            ),
            &format!(
                "{SH}
                ex:t1 a ex:Task ; ex:status \"open\" .
                ex:t2 a ex:Task ; ex:status \"bogus\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/t2");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#InConstraintComponent"
        );
    }

    #[test]
    fn node_reference_recursion() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:AddressShape a sh:NodeShape ;
                    sh:property [ sh:path ex:city ; sh:minCount 1 ] .
                ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
                    sh:property [ sh:path ex:address ; sh:node ex:AddressShape ] ."
            ),
            &format!(
                "{SH}
                ex:p1 a ex:Person ; ex:address ex:a1 . ex:a1 ex:city \"Paris\" .
                ex:p2 a ex:Person ; ex:address ex:a2 . ex:a2 ex:zip \"75000\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/a2");
        assert_eq!(
            report.results[0].path.as_deref(),
            Some("http://ex.org/city")
        );
    }

    #[test]
    fn target_subjects_of() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:OwnedShape a sh:NodeShape ; sh:targetSubjectsOf ex:owner ; sh:class ex:Owned ."
            ),
            &format!(
                "{SH}
                ex:car1 ex:owner ex:alice ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/car1");
    }

    #[test]
    fn severity_warning_does_not_break_conformance() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:NoteShape a sh:NodeShape ; sh:targetClass ex:Note ;
                    sh:property [ sh:path ex:summary ; sh:minCount 1 ;
                                  sh:severity sh:Warning ] ."
            ),
            &format!(
                "{SH}
                ex:n1 a ex:Note ."
            ),
        );
        // SHACL conforms = no sh:Violation results.
        assert!(report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].severity, Severity::Warning);
    }

    #[test]
    fn and_constraint_fails_when_one_shape_fails() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:AdultShape a sh:NodeShape ; sh:class ex:Adult .
                ex:NamedShape a sh:NodeShape ;
                    sh:property [ sh:path ex:name ; sh:minCount 1 ] .
                ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
                    sh:and ( ex:AdultShape ex:NamedShape ) ."
            ),
            &format!(
                "{SH}
                ex:alice a ex:Person, ex:Adult ; ex:name \"Alice\" .
                ex:bob a ex:Person, ex:Adult .
                ex:carol a ex:Person ; ex:name \"Carol\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        assert!(
            report
                .results
                .iter()
                .all(|r| r.component == "http://www.w3.org/ns/shacl#AndConstraintComponent")
        );
        let foci: Vec<_> = report
            .results
            .iter()
            .map(|r| r.focus_node.as_str())
            .collect();
        assert!(foci.contains(&"http://ex.org/bob"));
        assert!(foci.contains(&"http://ex.org/carol"));
    }

    #[test]
    fn or_constraint_passes_when_any_shape_matches() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:HasEmailShape a sh:NodeShape ;
                    sh:property [ sh:path ex:email ; sh:minCount 1 ] .
                ex:HasPhoneShape a sh:NodeShape ;
                    sh:property [ sh:path ex:phone ; sh:minCount 1 ] .
                ex:ContactShape a sh:NodeShape ; sh:targetClass ex:Contact ;
                    sh:or ( ex:HasEmailShape ex:HasPhoneShape ) ."
            ),
            &format!(
                "{SH}
                ex:c1 a ex:Contact ; ex:email \"a@x.org\" .
                ex:c2 a ex:Contact ; ex:phone \"+86\" .
                ex:c3 a ex:Contact ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/c3");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#OrConstraintComponent"
        );
    }

    #[test]
    fn not_constraint_violates_when_shape_matches() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:BannedStatusShape a sh:NodeShape ; sh:hasValue \"banned\" .
                ex:ItemShape a sh:NodeShape ; sh:targetClass ex:Item ;
                    sh:property [ sh:path ex:status ; sh:not ex:BannedStatusShape ] ."
            ),
            &format!(
                "{SH}
                ex:i1 a ex:Item ; ex:status \"active\" .
                ex:i2 a ex:Item ; ex:status \"banned\" ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/i2");
        assert_eq!(
            report.results[0].path.as_deref(),
            Some("http://ex.org/status")
        );
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#NotConstraintComponent"
        );
    }

    #[test]
    fn qualified_min_count_enforces_min_matches() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TeamShape a sh:NodeShape ; sh:targetClass ex:Team ;
                    sh:property [ sh:path ex:member ;
                                  sh:qualifiedValueShape [ sh:class ex:Engineer ] ;
                                  sh:qualifiedMinCount 2 ] ."
            ),
            &format!(
                "{SH}
                ex:t1 a ex:Team ; ex:member ex:e1, ex:e2 .
                ex:e1 a ex:Engineer . ex:e2 a ex:Engineer .
                ex:t2 a ex:Team ; ex:member ex:e3 .
                ex:e3 a ex:Engineer .
                ex:t3 a ex:Team ; ex:member ex:e4, ex:e5 .
                ex:e4 a ex:Engineer . ex:e5 a ex:Manager ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        assert!(
            report
                .results
                .iter()
                .all(|r| r.component
                    == "http://www.w3.org/ns/shacl#QualifiedMinCountConstraintComponent")
        );
        let foci: Vec<_> = report
            .results
            .iter()
            .map(|r| r.focus_node.as_str())
            .collect();
        assert!(foci.contains(&"http://ex.org/t2"));
        assert!(foci.contains(&"http://ex.org/t3"));
    }

    #[test]
    fn qualified_max_count_enforces_max_matches() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TeamShape a sh:NodeShape ; sh:targetClass ex:Team ;
                    sh:property [ sh:path ex:member ;
                                  sh:qualifiedValueShape [ sh:class ex:Engineer ] ;
                                  sh:qualifiedMaxCount 1 ] ."
            ),
            &format!(
                "{SH}
                ex:t1 a ex:Team ; ex:member ex:e1 . ex:e1 a ex:Engineer .
                ex:t2 a ex:Team ; ex:member ex:e1, ex:e2 .
                ex:e1 a ex:Engineer . ex:e2 a ex:Engineer ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].focus_node, "http://ex.org/t2");
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#QualifiedMaxCountConstraintComponent"
        );
    }

    #[test]
    fn qualified_value_shapes_disjoint_excludes_sibling_matches() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:TeamShape a sh:NodeShape ; sh:targetClass ex:Team ;
                    sh:property [ sh:path ex:member ;
                                  sh:qualifiedValueShape [ sh:class ex:Engineer ] ;
                                  sh:qualifiedValueShapesDisjoint true ;
                                  sh:qualifiedMinCount 1 ] ;
                    sh:property [ sh:path ex:member ;
                                  sh:qualifiedValueShape [ sh:class ex:Lead ] ;
                                  sh:qualifiedMaxCount 0 ] ."
            ),
            &format!(
                "{SH}
                ex:t1 a ex:Team ; ex:member ex:l1 .
                ex:l1 a ex:Lead, ex:Engineer ."
            ),
        );
        // l1 matches both sibling qualified shapes. It is excluded from the
        // disjoint engineer count (0 < qualifiedMinCount 1) and still counts
        // against the lead max (1 > qualifiedMaxCount 0).
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 2);
        let components: Vec<_> = report
            .results
            .iter()
            .map(|r| r.component.as_str())
            .collect();
        assert!(components.contains(&"http://www.w3.org/ns/shacl#QualifiedMinCountConstraintComponent"));
        assert!(components.contains(&"http://www.w3.org/ns/shacl#QualifiedMaxCountConstraintComponent"));
        assert!(report.results.iter().all(|r| r.focus_node == "http://ex.org/t1"));
    }

    #[test]
    fn closed_shape_allows_ignored_properties() {
        let dict = InMemoryDictionary::new();
        let report = validate(
            &dict,
            &format!(
                "{SH}
                ex:StrictShape a sh:NodeShape ; sh:targetClass ex:Strict ;
                    sh:closed true ; sh:ignoredProperties ( ex:comment ) ;
                    sh:property [ sh:path ex:name ; sh:minCount 1 ] ."
            ),
            &format!(
                "{SH}
                ex:s1 a ex:Strict ; ex:name \"ok\" ; ex:comment \"note\" .
                ex:s2 a ex:Strict ; ex:name \"x\" ; ex:extra 1 ."
            ),
        );
        assert!(!report.conforms);
        assert_eq!(report.results.len(), 1);
        assert_eq!(
            report.results[0].path.as_deref(),
            Some("http://ex.org/extra")
        );
        assert_eq!(
            report.results[0].component,
            "http://www.w3.org/ns/shacl#ClosedConstraintComponent"
        );
    }
}
