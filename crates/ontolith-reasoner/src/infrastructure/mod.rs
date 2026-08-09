//! Forward-chaining RDFS + OWL 2 RL materializer (L6).
//!
//! Fixpoint iteration over the complete rule set: the full W3C OWL 2 RL table
//! (OWL 2 Profiles §4.3, Tables 4–9: eq-*, prp-*, cls-*, cax-*, dt-*, scm-*)
//! plus the RDFS rules not subsumed by RL (rdf11-mt Table 1: rdfs1/4a/4b/6/8/
//! 10/12/13) and the derived cls-nothing3 — 87 rules in total (see
//! [`Rule`]). Axiomatic triples (prp-ap, cls-thing, cls-nothing1, dt-type1)
//! and the vacuous rdfs4a/4b rdfs:Resource typings are background entailments
//! excluded from `ReasoningReport::inferred_triples`. Guards:
//! `max_iterations` and `max_elapsed_ms` bound the loop and
//! `InferenceMode::Off` short-circuits.

mod shacl;

use crate::application::Reasoner;
use crate::domain::{MaterializeOutcome, ReasoningReport, ReasoningTask, Rule};
use ontolith_core::domain::{Iri, LiteralValue, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_storage::application::DictionaryCodec;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub use shacl::ShaclEngine;

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDFS_NS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const OWL_NS: &str = "http://www.w3.org/2002/07/owl#";
const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema#";

/// Default forward-chaining reasoner over the minimal rule set.
#[derive(Debug, Clone, Copy, Default)]
pub struct ForwardChainReasoner;

impl ForwardChainReasoner {
    pub fn new() -> Self {
        Self
    }
}

impl Reasoner for ForwardChainReasoner {
    fn materialize(
        &self,
        dict: &dyn DictionaryCodec,
        task: &ReasoningTask,
        input: &[Triple],
    ) -> Result<MaterializeOutcome, OntolithError> {
        let started = Instant::now();
        let mut closure: Vec<Triple> = input.to_vec();
        if !task.mode.is_enabled() {
            return Ok(MaterializeOutcome {
                triples: closure,
                report: ReasoningReport {
                    inferred_triples: 0,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    timed_out: false,
                    inconsistent: false,
                },
            });
        }

        // Background knowledge: the axiomatic OWL 2 RL triples (prp-ap,
        // cls-thing, cls-nothing1, dt-type1/rdfs1) plus their immediate rule
        // consequences (scm-cls/rdfs13/scm-sco closure). They are seeded once
        // and never counted as inferred — like RDFLib's `axioms=True` graph.
        let seeded_axioms = seed_axioms(dict, &mut closure);

        let mut timed_out = false;
        let mut inconsistent = false;
        for _ in 0..task.max_iterations.max(1) {
            if let Some(limit) = task.max_elapsed_ms
                && started.elapsed().as_millis() as u64 >= limit
            {
                timed_out = true;
                break;
            }
            let mut frontier = Vec::new();
            apply_rules(dict, &closure, &mut frontier, &mut inconsistent);
            let new_count = absorb_new(&mut closure, frontier);
            if new_count == 0 {
                break;
            }
        }

        // `inferred_triples` reports the domain-level closure delta: the
        // seeded axioms and the vacuous rdfs4a/4b rdfs:Resource typings are
        // background entailments (every IRI/blank node is a resource in
        // RDF 1.1), so they are not counted as inferences.
        let mut input_resources: HashSet<NodeId> = HashSet::new();
        for t in input {
            if t.predicate.as_str() == RDF_TYPE
                && t.object == Term::Iri(Iri::new(format!("{RDFS_NS}Resource")))
            {
                input_resources.insert(t.subject);
            }
        }
        let mut resource_background = 0usize;
        for t in &closure {
            if t.predicate.as_str() == RDF_TYPE
                && t.object == Term::Iri(Iri::new(format!("{RDFS_NS}Resource")))
                && !input_resources.contains(&t.subject)
            {
                resource_background += 1;
            }
        }
        let inferred = closure
            .len()
            .saturating_sub(input.len())
            .saturating_sub(seeded_axioms.len())
            .saturating_sub(resource_background);
        Ok(MaterializeOutcome {
            triples: closure,
            report: ReasoningReport {
                inferred_triples: inferred,
                elapsed_ms: started.elapsed().as_millis() as u64,
                timed_out,
                inconsistent,
            },
        })
    }

    fn supported_rules(&self) -> Vec<Rule> {
        // The complete RDFS + OWL 2 RL rule set (W3C OWL 2 Profiles §4.3
        // Tables 4–9, plus the RDFS rules not subsumed by RL).
        vec![
            // Equality.
            Rule::EqRef,
            Rule::SameAsSymmetric,
            Rule::SameAsTransitive,
            Rule::EqualityReplacementSubject,
            Rule::EqualityReplacementPredicate,
            Rule::EqualityReplacementObject,
            Rule::SameAsDifferentFrom,
            Rule::AllDifferentMembers,
            Rule::AllDifferentDistinctMembers,
            // Property axioms.
            Rule::AnnotationProperty,
            Rule::Domain,
            Rule::Range,
            Rule::FunctionalProperty,
            Rule::InverseFunctionalProperty,
            Rule::IrreflexiveProperty,
            Rule::SymmetricProperty,
            Rule::AsymmetricProperty,
            Rule::TransitiveProperty,
            Rule::SubPropertyOf,
            Rule::PropertyChain,
            Rule::EquivalentProperty,
            Rule::EquivalentPropertyReverse,
            Rule::PropertyDisjointWith,
            Rule::AllDisjointProperties,
            Rule::InverseOf,
            Rule::InverseOfReverse,
            Rule::HasKey,
            Rule::NegativePropertyAssertionObject,
            Rule::NegativePropertyAssertionValue,
            // Class expressions.
            Rule::ThingClass,
            Rule::NothingClass,
            Rule::NothingTyping,
            Rule::NothingSubClass,
            Rule::IntersectionOf,
            Rule::IntersectionOfTyping,
            Rule::UnionOf,
            Rule::ComplementClasses,
            Rule::SomeValuesFrom,
            Rule::SomeValuesFromTyping,
            Rule::AllValuesFrom,
            Rule::HasValue,
            Rule::HasValueTyping,
            Rule::MaxCardinalityZero,
            Rule::MaxCardinalityOne,
            Rule::MaxQualifiedCardinalityZero,
            Rule::MaxQualifiedCardinalityZeroThing,
            Rule::MaxQualifiedCardinalityOne,
            Rule::MaxQualifiedCardinalityOneThing,
            Rule::OneOf,
            // Class axioms.
            Rule::SubClassOf,
            Rule::EquivalentClass,
            Rule::EquivalentClassReverse,
            Rule::DisjointClasses,
            Rule::AllDisjointClasses,
            // Datatypes.
            Rule::DatatypeTyping,
            Rule::DatatypeLiteralTyping,
            Rule::DatatypeNotType,
            Rule::DatatypeEquality,
            Rule::DatatypeDifference,
            // Schema vocabulary.
            Rule::ClassSchema,
            Rule::SubClassOfTransitive,
            Rule::EquivalentClassSchema,
            Rule::EquivalentClassSchemaReverse,
            Rule::ObjectPropertySchema,
            Rule::DatatypePropertySchema,
            Rule::SubPropertyOfTransitive,
            Rule::EquivalentPropertySchema,
            Rule::EquivalentPropertySchemaReverse,
            Rule::DomainSchema,
            Rule::DomainSchemaSubproperty,
            Rule::RangeSchema,
            Rule::RangeSchemaSubproperty,
            Rule::HasValueSchema,
            Rule::SomeValuesSchema,
            Rule::SomeValuesSchemaSubproperty,
            Rule::AllValuesSchema,
            Rule::AllValuesSchemaSubproperty,
            Rule::IntersectionSchema,
            Rule::UnionSchema,
            // RDFS rules not subsumed by OWL 2 RL.
            Rule::DatatypeIriTyping,
            Rule::SubjectResource,
            Rule::ObjectResource,
            Rule::PropertyReflexive,
            Rule::ClassResource,
            Rule::ClassReflexive,
            Rule::ContainerMembership,
            Rule::DatatypeLiteral,
        ]
    }

    fn is_enabled(&self) -> bool {
        true
    }
}

/// dt-not-type: check a literal's lexical form against its declared datatype.
/// The compact `LiteralValue` variants are valid by construction; typed
/// literals with recognized XSD datatypes get a value-space check and anything
/// else is accepted.
fn literal_lexically_valid(value: &LiteralValue) -> bool {
    let LiteralValue::Typed {
        value: lex,
        datatype,
    } = value
    else {
        return true;
    };
    let dt = datatype.as_str();
    let v = lex.trim();
    if dt == format!("{XSD_NS}integer") {
        !v.is_empty() && v.parse::<i64>().is_ok()
    } else if dt == format!("{XSD_NS}decimal") {
        !v.is_empty() && (v.parse::<f64>().is_ok() || matches!(v, "INF" | "-INF" | "NaN"))
    } else if dt == format!("{XSD_NS}float") || dt == format!("{XSD_NS}double") {
        !v.is_empty() && (v.parse::<f64>().is_ok() || matches!(v, "INF" | "-INF" | "NaN"))
    } else if dt == format!("{XSD_NS}boolean") {
        matches!(v, "true" | "false" | "1" | "0")
    } else if dt == format!("{XSD_NS}date") {
        // yyyy-mm-dd with optional timezone; month/day ranges checked loosely.
        let re = |v: &str| -> bool {
            let bytes = v.as_bytes();
            bytes.len() >= 10
                && bytes[4] == b'-'
                && bytes[7] == b'-'
                && bytes[..4].iter().all(u8::is_ascii_digit)
                && bytes[5..7].iter().all(u8::is_ascii_digit)
                && bytes[8..10].iter().all(u8::is_ascii_digit)
        };
        let base = v.get(..10).map(re).unwrap_or(false);
        let rest = v.get(10..).unwrap_or("");
        base && (rest.is_empty()
            || rest == "Z"
            || (rest.starts_with('+') || rest.starts_with('-')) && rest.len() == 6)
    } else if dt == format!("{XSD_NS}dateTime") {
        // yyyy-mm-ddThh:mm:ss(.fff)?(Z|±hh:mm)?
        let bytes = v.as_bytes();
        let base_ok = bytes.len() >= 19
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':'
            && bytes[16] == b':'
            && bytes[..4].iter().all(u8::is_ascii_digit)
            && bytes[5..7].iter().all(u8::is_ascii_digit)
            && bytes[8..10].iter().all(u8::is_ascii_digit)
            && bytes[11..13].iter().all(u8::is_ascii_digit)
            && bytes[14..16].iter().all(u8::is_ascii_digit)
            && bytes[17..19].iter().all(u8::is_ascii_digit);
        if !base_ok {
            return false;
        }
        let rest = v.get(19..).unwrap_or("");
        if rest.is_empty() || rest == "Z" {
            return true;
        }
        if let Some(frac) = rest.strip_prefix('.') {
            let tz = frac.find(['Z', '+', '-']);
            let frac_ok = match tz {
                Some(i) => frac[..i].bytes().all(|b| b.is_ascii_digit()) && frac[i..].len() == 6,
                None => frac.bytes().all(|b| b.is_ascii_digit()),
            };
            return frac_ok;
        }
        (rest.starts_with('+') || rest.starts_with('-')) && rest.len() == 6
    } else {
        true
    }
}

/// Canonical data-value key for dt-eq: two literals sharing a key denote the
/// same data value (e.g. `"01"^^xsd:integer` and `1`). Non-recognized typed
/// literals compare by lexical form + datatype.
fn literal_value_key(value: &LiteralValue) -> Option<String> {
    match value {
        LiteralValue::String(v) => Some(format!("str:{v}")),
        LiteralValue::Lang { value, lang } => Some(format!("lang:{}:{value}", lang.as_str())),
        LiteralValue::Integer(v) => Some(format!("num:int:{v}")),
        LiteralValue::Decimal(v) => Some(format!("num:dec:{v:?}")),
        LiteralValue::Float(v) => Some(format!("num:float:{v:?}")),
        LiteralValue::Double(v) => Some(format!("num:double:{v:?}")),
        LiteralValue::Boolean(v) => Some(format!("bool:{v}")),
        LiteralValue::Typed { value, datatype } => {
            let dt = datatype.as_str();
            let trimmed = value.trim();
            if dt == format!("{XSD_NS}integer") {
                trimmed.parse::<i64>().ok().map(|n| format!("num:int:{n}"))
            } else if dt == format!("{XSD_NS}decimal") {
                trimmed
                    .parse::<f64>()
                    .ok()
                    .map(|n| format!("num:dec:{n:?}"))
            } else if dt == format!("{XSD_NS}float") {
                trimmed
                    .parse::<f32>()
                    .ok()
                    .map(|n| format!("num:float:{n:?}"))
            } else if dt == format!("{XSD_NS}double") {
                trimmed
                    .parse::<f64>()
                    .ok()
                    .map(|n| format!("num:double:{n:?}"))
            } else if dt == format!("{XSD_NS}boolean") {
                match trimmed {
                    "true" | "1" => Some("bool:true".to_owned()),
                    "false" | "0" => Some("bool:false".to_owned()),
                    _ => None,
                }
            } else {
                Some(format!("typed:{dt}:{value}"))
            }
        }
    }
}

/// Stable dedup preserving first-occurrence order (Iri is not Ord).
fn dedup_keep_order(items: Vec<Iri>) -> Vec<Iri> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for item in items {
        if !seen.iter().any(|s| s == item.as_str()) {
            seen.push(item.as_str().to_owned());
            out.push(item);
        }
    }
    out
}

/// Seed the axiomatic OWL 2 RL background triples (prp-ap, cls-thing,
/// cls-nothing1, dt-type1/rdfs1) plus their immediate rule consequences
/// (scm-cls self-schema, rdfs13 datatype ⊑ rdfs:Literal, scm-sco closure).
/// Returns the triples newly added to `closure`.
fn seed_axioms(dict: &dyn DictionaryCodec, closure: &mut Vec<Triple>) -> Vec<Triple> {
    let rdf_type = Iri::new(RDF_TYPE);
    let rdfs = |name: &str| -> Iri { Iri::new(format!("{RDFS_NS}{name}")) };
    let owl = |name: &str| -> Iri { Iri::new(format!("{OWL_NS}{name}")) };
    let mut axioms: Vec<Triple> = Vec::new();
    let mut push = |s: &str, p: &Iri, o: Term| {
        let t = Triple::new(dict.encode_node(s), p.clone(), o);
        if !axioms.contains(&t) {
            axioms.push(t);
        }
    };
    // prp-ap: built-in annotation properties are owl:AnnotationProperty.
    for name in ["label", "comment", "seeAlso", "isDefinedBy"] {
        push(
            &format!("{RDFS_NS}{name}"),
            &rdf_type,
            Term::Iri(owl("AnnotationProperty")),
        );
    }
    for name in [
        "versionInfo",
        "deprecated",
        "priorVersion",
        "backwardCompatibleWith",
        "incompatibleWith",
        "imports",
    ] {
        push(
            &format!("{OWL_NS}{name}"),
            &rdf_type,
            Term::Iri(owl("AnnotationProperty")),
        );
    }
    // cls-thing / cls-nothing1: owl:Thing / owl:Nothing are classes.
    for c in [format!("{OWL_NS}Thing"), format!("{OWL_NS}Nothing")] {
        push(&c, &rdf_type, Term::Iri(owl("Class")));
        // scm-cls closure: c ⊑ c, c ≡ c.
        push(&c, &rdfs("subClassOf"), Term::Iri(Iri::new(c.clone())));
        push(&c, &owl("equivalentClass"), Term::Iri(Iri::new(c.clone())));
    }
    // scm-cls: owl:Nothing ⊑ owl:Thing.
    push(
        &format!("{OWL_NS}Nothing"),
        &rdfs("subClassOf"),
        Term::Iri(owl("Thing")),
    );
    // dt-type1 / rdfs1: supported datatypes are rdfs:Datatype; rdfs13: dt ⊑ rdfs:Literal.
    for dt in [
        format!("{}string", XSD_NS),
        format!("{}boolean", XSD_NS),
        format!("{}decimal", XSD_NS),
        format!("{}integer", XSD_NS),
        format!("{}float", XSD_NS),
        format!("{}double", XSD_NS),
        format!("{}dateTime", XSD_NS),
        format!("{}date", XSD_NS),
        format!("{}time", XSD_NS),
        format!("{}anyURI", XSD_NS),
        format!("{RDF_NS}langString"),
    ] {
        push(&dt, &rdf_type, Term::Iri(rdfs("Datatype")));
        push(&dt, &rdfs("subClassOf"), Term::Iri(rdfs("Literal")));
    }
    let mut added = Vec::new();
    for t in axioms {
        if !closure.contains(&t) {
            added.push(t.clone());
            closure.push(t);
        }
    }
    added
}

fn apply_rules(
    dict: &dyn DictionaryCodec,
    closure: &[Triple],
    frontier: &mut Vec<Triple>,
    inconsistent: &mut bool,
) {
    let node_iri = |id: NodeId| -> Option<Iri> {
        let value = dict.decode_node(id)?;
        Iri::parse(value).ok()
    };
    let iri_of = |term: &Term| -> Option<Iri> {
        if let Term::Iri(iri) = term {
            Some(iri.clone())
        } else {
            None
        }
    };
    let rdfs = |name: &str| -> Iri { Iri::new(format!("{RDFS_NS}{name}")) };
    let owl = |name: &str| -> Iri { Iri::new(format!("{OWL_NS}{name}")) };
    let rdf_type = Iri::new(RDF_TYPE);
    let node_term = |id: NodeId| -> Option<Term> {
        let value = dict.decode_node(id)?;
        if value.starts_with("_:") {
            Some(Term::BlankNode(id))
        } else {
            Some(Term::Iri(Iri::parse(value).ok()?))
        }
    };
    let subject_node = |term: &Term| -> Option<NodeId> {
        match term {
            Term::Iri(iri) => Some(dict.encode_node(iri.as_str())),
            Term::BlankNode(id) => Some(*id),
            Term::Literal(_) => None,
        }
    };
    let node_term_from_term = |term: &Term| -> Option<Term> {
        match term {
            Term::Iri(iri) => Some(Term::Iri(iri.clone())),
            Term::BlankNode(id) => Some(Term::BlankNode(*id)),
            Term::Literal(_) => None,
        }
    };

    // Index rule statements by IRI-level pairs.
    let subclass: Vec<(Iri, Iri)> = closure
        .iter()
        .filter_map(|t| {
            let (s, o) = (node_iri(t.subject)?, iri_of(&t.object)?);
            (t.predicate == rdfs("subClassOf")).then_some((s, o))
        })
        .collect();
    let subprop: Vec<(Iri, Iri)> = closure
        .iter()
        .filter_map(|t| {
            let (s, o) = (node_iri(t.subject)?, iri_of(&t.object)?);
            (t.predicate == rdfs("subPropertyOf")).then_some((s, o))
        })
        .collect();
    let inverse: Vec<(Iri, Iri)> = closure
        .iter()
        .filter_map(|t| {
            let (s, o) = (node_iri(t.subject)?, iri_of(&t.object)?);
            (t.predicate == owl("inverseOf")).then_some((s, o))
        })
        .collect();

    // rdfs5: subClassOf transitivity.
    for (x, y) in &subclass {
        for (_, z) in subclass.iter().filter(|(a, _)| a == y) {
            frontier.push(Triple::new(
                dict.encode_node(x.as_str()),
                rdfs("subClassOf"),
                Term::Iri(z.clone()),
            ));
        }
    }

    // rdfs6: subPropertyOf transitivity + rdfs9: property application.
    for (p, q) in &subprop {
        for (_, r) in subprop.iter().filter(|(a, _)| a == q) {
            frontier.push(Triple::new(
                dict.encode_node(p.as_str()),
                rdfs("subPropertyOf"),
                Term::Iri(r.clone()),
            ));
        }
        for t in closure {
            if &t.predicate == p {
                frontier.push(Triple::new(t.subject, q.clone(), t.object.clone()));
            }
        }
    }

    // prp-eqp1/prp-eqp2 + scm-eqp1: equivalentProperty → mutual subPropertyOf;
    // scm-eqp2: mutual subPropertyOf → equivalentProperty.
    let equivalent_property: Vec<(Iri, Iri)> = closure
        .iter()
        .filter_map(|t| {
            if t.predicate != owl("equivalentProperty") {
                return None;
            }
            let (s, o) = (node_iri(t.subject)?, iri_of(&t.object)?);
            Some((s, o))
        })
        .collect();
    for (p1, p2) in &equivalent_property {
        frontier.push(Triple::new(
            dict.encode_node(p1.as_str()),
            rdfs("subPropertyOf"),
            Term::Iri(p2.clone()),
        ));
        frontier.push(Triple::new(
            dict.encode_node(p2.as_str()),
            rdfs("subPropertyOf"),
            Term::Iri(p1.clone()),
        ));
    }
    for (p1, p2) in &subprop {
        if subprop.iter().any(|(a, b)| a == p2 && b == p1) {
            frontier.push(Triple::new(
                dict.encode_node(p1.as_str()),
                owl("equivalentProperty"),
                Term::Iri(p2.clone()),
            ));
        }
    }

    // scm-op / scm-dp (+ rdf:Property): p ⊑ p and p ≡ p for typed properties.
    let mut property_schema: Vec<Iri> = closure
        .iter()
        .filter_map(|t| {
            if t.predicate != rdf_type {
                return None;
            }
            let class = iri_of(&t.object)?;
            if class == owl("ObjectProperty")
                || class == owl("DatatypeProperty")
                || class == Iri::new(format!("{RDF_NS}Property"))
            {
                node_iri(t.subject)
            } else {
                None
            }
        })
        .collect();
    property_schema = dedup_keep_order(property_schema);
    for p in &property_schema {
        frontier.push(Triple::new(
            dict.encode_node(p.as_str()),
            rdfs("subPropertyOf"),
            Term::Iri(p.clone()),
        ));
        frontier.push(Triple::new(
            dict.encode_node(p.as_str()),
            owl("equivalentProperty"),
            Term::Iri(p.clone()),
        ));
    }

    // scm-cls: ?c rdf:type owl:Class → c ⊑ c, c ≡ c, c ⊑ owl:Thing, owl:Nothing ⊑ c.
    let mut classes: Vec<Iri> = closure
        .iter()
        .filter_map(|t| {
            (t.predicate == rdf_type && t.object == Term::Iri(owl("Class")))
                .then(|| node_iri(t.subject))
                .flatten()
        })
        .collect();
    classes = dedup_keep_order(classes);
    for c in &classes {
        frontier.push(Triple::new(
            dict.encode_node(c.as_str()),
            rdfs("subClassOf"),
            Term::Iri(c.clone()),
        ));
        frontier.push(Triple::new(
            dict.encode_node(c.as_str()),
            owl("equivalentClass"),
            Term::Iri(c.clone()),
        ));
        frontier.push(Triple::new(
            dict.encode_node(c.as_str()),
            rdfs("subClassOf"),
            Term::Iri(owl("Thing")),
        ));
        frontier.push(Triple::new(
            dict.encode_node(owl("Nothing").as_str()),
            rdfs("subClassOf"),
            Term::Iri(c.clone()),
        ));
    }

    // scm-eqc1 + cax-eqc1/2: equivalentClass → mutual subClassOf and type
    // propagation; scm-eqc2: mutual subClassOf → equivalentClass.
    let equivalent_class: Vec<(Iri, Iri)> = closure
        .iter()
        .filter_map(|t| {
            if t.predicate != owl("equivalentClass") {
                return None;
            }
            let (s, o) = (node_iri(t.subject)?, iri_of(&t.object)?);
            Some((s, o))
        })
        .collect();
    for (c1, c2) in &equivalent_class {
        frontier.push(Triple::new(
            dict.encode_node(c1.as_str()),
            rdfs("subClassOf"),
            Term::Iri(c2.clone()),
        ));
        frontier.push(Triple::new(
            dict.encode_node(c2.as_str()),
            rdfs("subClassOf"),
            Term::Iri(c1.clone()),
        ));
    }
    for (c1, c2) in &equivalent_class {
        for t in closure {
            if t.predicate != rdf_type {
                continue;
            }
            if t.object == Term::Iri(c1.clone()) {
                frontier.push(Triple::new(
                    t.subject,
                    rdf_type.clone(),
                    Term::Iri(c2.clone()),
                ));
            } else if t.object == Term::Iri(c2.clone()) {
                frontier.push(Triple::new(
                    t.subject,
                    rdf_type.clone(),
                    Term::Iri(c1.clone()),
                ));
            }
        }
    }
    for (c1, c2) in &subclass {
        if subclass.iter().any(|(a, b)| a == c2 && b == c1) {
            frontier.push(Triple::new(
                dict.encode_node(c1.as_str()),
                owl("equivalentClass"),
                Term::Iri(c2.clone()),
            ));
        }
    }

    // scm-dom2 / scm-rng2: domain/range propagate down subPropertyOf.
    for (p1, p2) in &subprop {
        for t in closure {
            if t.predicate == rdfs("domain") && node_iri(t.subject).as_ref() == Some(p2) {
                frontier.push(Triple::new(
                    dict.encode_node(p1.as_str()),
                    rdfs("domain"),
                    t.object.clone(),
                ));
            }
            if t.predicate == rdfs("range") && node_iri(t.subject).as_ref() == Some(p2) {
                frontier.push(Triple::new(
                    dict.encode_node(p1.as_str()),
                    rdfs("range"),
                    t.object.clone(),
                ));
            }
        }
    }

    // scm-dom1 / scm-rng1: domain/range propagate up subClassOf.
    for t in closure {
        if t.predicate != rdfs("domain") && t.predicate != rdfs("range") {
            continue;
        }
        let Some(c1) = iri_of(&t.object) else {
            continue;
        };
        for (_, c2) in subclass.iter().filter(|(a, _)| *a == c1) {
            frontier.push(Triple::new(
                t.subject,
                t.predicate.clone(),
                Term::Iri(c2.clone()),
            ));
        }
    }

    // prp-inv1: inverse property application.
    for (p, q) in &inverse {
        for t in closure {
            if &t.predicate == p
                && let Some(o) = iri_of(&t.object)
                && let Some(s) = node_iri(t.subject)
            {
                frontier.push(Triple::new(
                    dict.encode_node(o.as_str()),
                    q.clone(),
                    Term::Iri(s),
                ));
            }
        }
    }

    // prp-inv2: inverse property reverse application.
    for (p, q) in &inverse {
        for t in closure {
            if &t.predicate == q
                && let Some(o) = iri_of(&t.object)
                && let Some(s) = node_iri(t.subject)
            {
                frontier.push(Triple::new(
                    dict.encode_node(o.as_str()),
                    p.clone(),
                    Term::Iri(s),
                ));
            }
        }
    }

    // prp-symp: symmetric property application.
    let symmetric: Vec<Iri> = closure
        .iter()
        .filter_map(|t| {
            (t.predicate == rdf_type && t.object == Term::Iri(owl("SymmetricProperty")))
                .then(|| node_iri(t.subject))
                .flatten()
        })
        .collect();
    for p in &symmetric {
        for t in closure {
            if &t.predicate == p
                && let Some(s) = node_iri(t.subject)
                && let Some(o) = iri_of(&t.object)
            {
                frontier.push(Triple::new(
                    dict.encode_node(o.as_str()),
                    p.clone(),
                    Term::Iri(s),
                ));
            }
        }
    }

    // prp-trp: transitive property application.
    let transitive: Vec<Iri> = closure
        .iter()
        .filter_map(|t| {
            (t.predicate == rdf_type && t.object == Term::Iri(owl("TransitiveProperty")))
                .then(|| node_iri(t.subject))
                .flatten()
        })
        .collect();
    for p in &transitive {
        for a in closure {
            if &a.predicate == p
                && let Some(ao) = iri_of(&a.object)
            {
                for b in closure {
                    if &b.predicate == p
                        && node_iri(b.subject).as_ref() == Some(&ao)
                        && let Some(bo) = iri_of(&b.object)
                    {
                        frontier.push(Triple::new(a.subject, p.clone(), Term::Iri(bo)));
                    }
                }
            }
        }
    }

    // prp-fp: functional property — same subject with two values → values sameAs.
    let functional: Vec<Iri> = closure
        .iter()
        .filter_map(|t| {
            (t.predicate == rdf_type && t.object == Term::Iri(owl("FunctionalProperty")))
                .then(|| node_iri(t.subject))
                .flatten()
        })
        .collect();
    for p in &functional {
        let mut by_subject: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for t in closure {
            if &t.predicate == p
                && let Some(o) = subject_node(&t.object)
            {
                by_subject.entry(t.subject).or_default().push(o);
            }
        }
        for objects in by_subject.values() {
            for i in 0..objects.len() {
                for j in (i + 1)..objects.len() {
                    if let Some(y2_term) = node_term(objects[j]) {
                        frontier.push(Triple::new(objects[i], owl("sameAs"), y2_term));
                    }
                }
            }
        }
    }

    // prp-ifp: inverse functional property — same value with two subjects → subjects sameAs.
    let inverse_functional: Vec<Iri> = closure
        .iter()
        .filter_map(|t| {
            (t.predicate == rdf_type && t.object == Term::Iri(owl("InverseFunctionalProperty")))
                .then(|| node_iri(t.subject))
                .flatten()
        })
        .collect();
    for p in &inverse_functional {
        let mut by_value: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for t in closure {
            if &t.predicate == p
                && let Some(o) = subject_node(&t.object)
            {
                by_value.entry(o).or_default().push(t.subject);
            }
        }
        for subjects in by_value.values() {
            for i in 0..subjects.len() {
                for j in (i + 1)..subjects.len() {
                    if let Some(x2_term) = node_term(subjects[j]) {
                        frontier.push(Triple::new(subjects[i], owl("sameAs"), x2_term));
                    }
                }
            }
        }
    }

    // rdfs7/rdfs8: domain and range typing.
    for t in closure {
        let domain_class = closure.iter().find_map(|r| {
            (r.predicate == rdfs("domain") && node_iri(r.subject).as_ref() == Some(&t.predicate))
                .then(|| iri_of(&r.object))
                .flatten()
        });
        if let Some(class) = domain_class {
            frontier.push(Triple::new(t.subject, rdf_type.clone(), Term::Iri(class)));
        }
        let range_class = closure.iter().find_map(|r| {
            (r.predicate == rdfs("range") && node_iri(r.subject).as_ref() == Some(&t.predicate))
                .then(|| iri_of(&r.object))
                .flatten()
        });
        if let Some(class) = range_class
            && let Some(o) = iri_of(&t.object)
        {
            frontier.push(Triple::new(
                dict.encode_node(o.as_str()),
                rdf_type.clone(),
                Term::Iri(class),
            ));
        }
    }

    // rdfs4a/4b: subjects and (IRI/blank-node) objects are rdfs:Resource.
    let rdfs_resource = Term::Iri(Iri::new(format!("{RDFS_NS}Resource")));
    for t in closure {
        if t.object.kind() != "literal" {
            frontier.push(Triple::new(
                t.subject,
                rdf_type.clone(),
                rdfs_resource.clone(),
            ));
        }
    }
    let mut resource_nodes: Vec<NodeId> = Vec::new();
    for t in closure {
        if t.object.kind() != "literal"
            && let Some(o) = subject_node(&t.object)
            && !resource_nodes.contains(&o)
        {
            resource_nodes.push(o);
        }
    }
    for o in resource_nodes {
        frontier.push(Triple::new(o, rdf_type.clone(), rdfs_resource.clone()));
    }

    // rdfs6: p rdf:type rdf:Property → p rdfs:subPropertyOf p.
    // rdfs12: p rdf:type rdfs:ContainerMembershipProperty → p rdfs:subPropertyOf rdfs:member.
    // rdfs8/rdfs10: c rdf:type rdfs:Class → c rdfs:subClassOf rdfs:Resource / c.
    // rdfs13: d rdf:type rdfs:Datatype → d rdfs:subClassOf rdfs:Literal.
    let rdf_property = Iri::new(format!("{RDF_NS}Property"));
    let rdfs_class = rdfs("Class");
    let rdfs_cmp = rdfs("ContainerMembershipProperty");
    let rdfs_datatype = rdfs("Datatype");
    let rdfs_member = rdfs("member");
    let rdfs_literal = rdfs("Literal");
    for t in closure {
        if t.predicate != rdf_type {
            continue;
        }
        let Some(c) = node_iri(t.subject) else {
            continue;
        };
        if t.object == Term::Iri(rdf_property.clone()) {
            frontier.push(Triple::new(
                dict.encode_node(c.as_str()),
                rdfs("subPropertyOf"),
                Term::Iri(c.clone()),
            ));
        }
        if t.object == Term::Iri(rdfs_cmp.clone()) {
            frontier.push(Triple::new(
                dict.encode_node(c.as_str()),
                rdfs("subPropertyOf"),
                Term::Iri(rdfs_member.clone()),
            ));
        }
        if t.object == Term::Iri(rdfs_class.clone()) {
            frontier.push(Triple::new(
                dict.encode_node(c.as_str()),
                rdfs("subClassOf"),
                rdfs_resource.clone(),
            ));
            frontier.push(Triple::new(
                dict.encode_node(c.as_str()),
                rdfs("subClassOf"),
                Term::Iri(c.clone()),
            ));
        }
        if t.object == Term::Iri(rdfs_datatype.clone()) {
            frontier.push(Triple::new(
                dict.encode_node(c.as_str()),
                rdfs("subClassOf"),
                Term::Iri(rdfs_literal.clone()),
            ));
        }
    }

    // dt-eq: literals with the same data value are interchangeable as objects.
    // (Bounded to the graph's literal objects, mirroring RDFLib's hidden sameAs.)
    let mut literal_groups: Vec<(String, Vec<Term>)> = Vec::new();
    for t in closure {
        if let Term::Literal(value) = &t.object
            && let Some(key) = literal_value_key(value)
        {
            match literal_groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, terms)) => {
                    if !terms.contains(&t.object) {
                        terms.push(t.object.clone());
                    }
                }
                None => literal_groups.push((key, vec![t.object.clone()])),
            }
        }
    }
    for (_, terms) in &literal_groups {
        for i in 0..terms.len() {
            for j in (i + 1)..terms.len() {
                let (lt1, lt2) = (&terms[i], &terms[j]);
                for t in closure {
                    if &t.object == lt1 {
                        frontier.push(Triple::new(t.subject, t.predicate.clone(), lt2.clone()));
                    } else if &t.object == lt2 {
                        frontier.push(Triple::new(t.subject, t.predicate.clone(), lt1.clone()));
                    }
                }
            }
        }
    }

    // cax-sco: subclass application on rdf:type.
    for t in closure {
        if t.predicate == rdf_type
            && let Some(class) = iri_of(&t.object)
        {
            for (c, d) in &subclass {
                if c == &class {
                    frontier.push(Triple::new(
                        t.subject,
                        rdf_type.clone(),
                        Term::Iri(d.clone()),
                    ));
                }
            }
        }
    }

    // Restriction axioms indexed by restriction node: (restriction, property) and
    // (restriction, value class). Restriction subjects may be blank nodes.
    let on_property: Vec<(Term, Iri)> = closure
        .iter()
        .filter_map(|t| {
            if t.predicate != owl("onProperty") {
                return None;
            }
            let (restr, p) = (node_term(t.subject)?, iri_of(&t.object)?);
            Some((restr, p))
        })
        .collect();
    let some_values: Vec<(Term, Iri)> = closure
        .iter()
        .filter_map(|t| {
            if t.predicate != owl("someValuesFrom") {
                return None;
            }
            let (restr, c) = (node_term(t.subject)?, iri_of(&t.object)?);
            Some((restr, c))
        })
        .collect();
    let all_values: Vec<(Term, Iri)> = closure
        .iter()
        .filter_map(|t| {
            if t.predicate != owl("allValuesFrom") {
                return None;
            }
            let (restr, c) = (node_term(t.subject)?, iri_of(&t.object)?);
            Some((restr, c))
        })
        .collect();
    // Restriction nodes with owl:maxCardinality "1"^^xsd:nonNegativeInteger.
    let max_cardinality_one: Vec<(Term, Iri)> = closure
        .iter()
        .filter(|t| {
            t.predicate == owl("maxCardinality")
                && t.object == Term::Literal(LiteralValue::Integer(1))
        })
        .filter_map(|t| {
            let restr = node_term(t.subject)?;
            let p = on_property
                .iter()
                .find(|(r, _)| *r == restr)
                .map(|(_, p)| p.clone())?;
            Some((restr, p))
        })
        .collect();
    // Restriction nodes with owl:hasValue; the value may be an IRI, blank node, or literal.
    let has_value: Vec<(Term, Term)> = closure
        .iter()
        .filter(|t| t.predicate == owl("hasValue"))
        .filter_map(|t| {
            let restr = node_term(t.subject)?;
            Some((restr, t.object.clone()))
        })
        .collect();
    // Restriction nodes with owl:maxCardinality "0"^^xsd:nonNegativeInteger.
    let max_cardinality_zero: Vec<(Term, Iri)> = closure
        .iter()
        .filter(|t| {
            t.predicate == owl("maxCardinality")
                && t.object == Term::Literal(LiteralValue::Integer(0))
        })
        .filter_map(|t| {
            let restr = node_term(t.subject)?;
            let p = on_property
                .iter()
                .find(|(r, _)| *r == restr)
                .map(|(_, p)| p.clone())?;
            Some((restr, p))
        })
        .collect();
    // owl:onClass (qualified cardinality) and the qualified-cardinality indexes.
    let on_class: Vec<(Term, Iri)> = closure
        .iter()
        .filter_map(|t| {
            if t.predicate != owl("onClass") {
                return None;
            }
            let (restr, c) = (node_term(t.subject)?, iri_of(&t.object)?);
            Some((restr, c))
        })
        .collect();
    let qualified_cardinality = |value: i64| -> Vec<(Term, Iri, Iri)> {
        closure
            .iter()
            .filter(|t| {
                t.predicate == owl("maxQualifiedCardinality")
                    && t.object == Term::Literal(LiteralValue::Integer(value))
            })
            .filter_map(|t| {
                let restr = node_term(t.subject)?;
                let p = on_property
                    .iter()
                    .find(|(r, _)| *r == restr)
                    .map(|(_, p)| p.clone())?;
                let c = on_class
                    .iter()
                    .find(|(r, _)| *r == restr)
                    .map(|(_, c)| c.clone())
                    .unwrap_or_else(|| owl("Thing"));
                Some((restr, p, c))
            })
            .collect()
    };
    let max_qualified_zero: Vec<(Term, Iri, Iri)> = qualified_cardinality(0);
    let max_qualified_one: Vec<(Term, Iri, Iri)> = qualified_cardinality(1);

    // cls-svf1: x rdf:type restr ∧ restr onProperty p ∧ restr someValuesFrom c ∧ x p y → y rdf:type c.
    for t in closure {
        if t.predicate != rdf_type {
            continue;
        }
        let Some(restr) = node_term_from_term(&t.object) else {
            continue;
        };
        let Some(p) = on_property
            .iter()
            .find(|(r, _)| *r == restr)
            .map(|(_, p)| p.clone())
        else {
            continue;
        };
        let Some(c) = some_values
            .iter()
            .find(|(r, _)| *r == restr)
            .map(|(_, c)| c.clone())
        else {
            continue;
        };
        for u in closure {
            if u.predicate == p
                && u.subject == t.subject
                && let Some(y) = subject_node(&u.object)
            {
                frontier.push(Triple::new(y, rdf_type.clone(), Term::Iri(c.clone())));
            }
        }
    }

    // cls-svf2: x p y ∧ y rdf:type c ∧ restr onProperty p ∧ restr someValuesFrom c → x rdf:type restr.
    for (restr, p) in &on_property {
        let Some(c) = some_values
            .iter()
            .find(|(r, _)| r == restr)
            .map(|(_, c)| c.clone())
        else {
            continue;
        };
        for t in closure {
            if &t.predicate != p {
                continue;
            }
            let Some(y) = subject_node(&t.object) else {
                continue;
            };
            let typed = closure.iter().any(|s| {
                s.predicate == rdf_type && s.subject == y && s.object == Term::Iri(c.clone())
            });
            if typed {
                frontier.push(Triple::new(t.subject, rdf_type.clone(), restr.clone()));
            }
        }
    }

    // cls-avf: x rdf:type restr ∧ restr onProperty p ∧ restr allValuesFrom c ∧ x p y → y rdf:type c.
    for t in closure {
        if t.predicate != rdf_type {
            continue;
        }
        let Some(restr) = node_term_from_term(&t.object) else {
            continue;
        };
        let Some(p) = on_property
            .iter()
            .find(|(r, _)| *r == restr)
            .map(|(_, p)| p.clone())
        else {
            continue;
        };
        let Some(c) = all_values
            .iter()
            .find(|(r, _)| *r == restr)
            .map(|(_, c)| c.clone())
        else {
            continue;
        };
        for u in closure {
            if u.predicate == p
                && u.subject == t.subject
                && let Some(y) = subject_node(&u.object)
            {
                frontier.push(Triple::new(y, rdf_type.clone(), Term::Iri(c.clone())));
            }
        }
    }

    // cls-hv1: x rdf:type restr ∧ restr onProperty p ∧ restr hasValue y → x p y.
    for t in closure {
        if t.predicate != rdf_type {
            continue;
        }
        let Some(restr) = node_term_from_term(&t.object) else {
            continue;
        };
        let Some(p) = on_property
            .iter()
            .find(|(r, _)| *r == restr)
            .map(|(_, p)| p.clone())
        else {
            continue;
        };
        let Some(value) = has_value
            .iter()
            .find(|(r, _)| *r == restr)
            .map(|(_, v)| v.clone())
        else {
            continue;
        };
        frontier.push(Triple::new(t.subject, p, value));
    }

    // cls-hv2: x p y ∧ restr onProperty p ∧ restr hasValue y → x rdf:type restr.
    for (restr, value) in &has_value {
        let Some(p) = on_property
            .iter()
            .find(|(r, _)| r == restr)
            .map(|(_, p)| p.clone())
        else {
            continue;
        };
        for t in closure {
            if t.predicate == p && &t.object == value {
                frontier.push(Triple::new(t.subject, rdf_type.clone(), restr.clone()));
            }
        }
    }

    // cls-maxc2: x rdf:type (p max 1) ∧ x p y1 ∧ x p y2 → y1 owl:sameAs y2.
    for (restr, p) in &max_cardinality_one {
        let mut by_subject: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for t in closure {
            if &t.predicate == p
                && let Some(o) = subject_node(&t.object)
            {
                by_subject.entry(t.subject).or_default().push(o);
            }
        }
        for (x, objects) in &by_subject {
            let typed = closure
                .iter()
                .any(|s| s.subject == *x && s.predicate == rdf_type && s.object == *restr);
            if !typed {
                continue;
            }
            for i in 0..objects.len() {
                for j in (i + 1)..objects.len() {
                    if let Some(y2_term) = node_term(objects[j]) {
                        frontier.push(Triple::new(objects[i], owl("sameAs"), y2_term));
                    }
                }
            }
        }
    }

    // cls-maxqc3: (p max 1 c) ∧ u type (p max 1 c) ∧ u p y1/y2 ∧ y1/y2 type c → y1 sameAs y2.
    // cls-maxqc4: same with c = owl:Thing (every value qualifies).
    for (restr, p, c) in &max_qualified_one {
        let mut by_subject: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for t in closure {
            if &t.predicate != p {
                continue;
            }
            let Some(o) = subject_node(&t.object) else {
                continue;
            };
            let qualified = c == &owl("Thing")
                || closure.iter().any(|s| {
                    s.subject == o && s.predicate == rdf_type && s.object == Term::Iri(c.clone())
                });
            if qualified {
                by_subject.entry(t.subject).or_default().push(o);
            }
        }
        for (x, objects) in &by_subject {
            let typed = closure
                .iter()
                .any(|s| s.subject == *x && s.predicate == rdf_type && s.object == *restr);
            if !typed {
                continue;
            }
            for i in 0..objects.len() {
                for j in (i + 1)..objects.len() {
                    if let Some(y2_term) = node_term(objects[j]) {
                        frontier.push(Triple::new(objects[i], owl("sameAs"), y2_term));
                    }
                }
            }
        }
    }

    // Members of an RDF list starting at `start`, preserving term kinds.
    let list_terms = |start: &Term| -> Vec<Term> {
        let mut out = Vec::new();
        let mut cur = start.clone();
        let mut seen: Vec<Term> = Vec::new();
        loop {
            if seen.contains(&cur) {
                break;
            }
            seen.push(cur.clone());
            let Some(cur_node) = subject_node(&cur) else {
                break;
            };
            let first = closure
                .iter()
                .find(|t| t.subject == cur_node && t.predicate.as_str() == RDF_FIRST)
                .map(|t| t.object.clone());
            let rest = closure
                .iter()
                .find(|t| t.subject == cur_node && t.predicate.as_str() == RDF_REST)
                .map(|t| t.object.clone());
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
    };
    // IRI-only members (class/property lists).
    let list_members = |start: &Term| -> Vec<Iri> {
        list_terms(start)
            .into_iter()
            .filter_map(|term| iri_of(&term))
            .collect()
    };

    // List-valued class expressions: (restriction node, list members).
    let intersection_lists: Vec<(Term, Vec<Iri>)> = closure
        .iter()
        .filter(|t| t.predicate == owl("intersectionOf"))
        .filter_map(|t| {
            let restr = node_term(t.subject)?;
            let members = list_members(&t.object);
            (!members.is_empty()).then_some((restr, members))
        })
        .collect();
    let union_lists: Vec<(Term, Vec<Iri>)> = closure
        .iter()
        .filter(|t| t.predicate == owl("unionOf"))
        .filter_map(|t| {
            let restr = node_term(t.subject)?;
            let members = list_members(&t.object);
            (!members.is_empty()).then_some((restr, members))
        })
        .collect();
    let has_keys: Vec<(Term, Vec<Iri>)> = closure
        .iter()
        .filter(|t| t.predicate == owl("hasKey"))
        .filter_map(|t| {
            let class = node_term(t.subject)?;
            let members = list_members(&t.object);
            (!members.is_empty()).then_some((class, members))
        })
        .collect();

    // cls-int1: x rdf:type (C1 ∩ … ∩ Cn) → x rdf:type Ci for every member.
    for t in closure {
        if t.predicate != rdf_type {
            continue;
        }
        let Some(restr) = node_term_from_term(&t.object) else {
            continue;
        };
        let Some(members) = intersection_lists
            .iter()
            .find(|(r, _)| *r == restr)
            .map(|(_, m)| m.clone())
        else {
            continue;
        };
        for c in members {
            frontier.push(Triple::new(t.subject, rdf_type.clone(), Term::Iri(c)));
        }
    }

    // cls-int2: x rdf:type Ci for all members → x rdf:type (C1 ∩ … ∩ Cn).
    for (restr, members) in &intersection_lists {
        let candidates: Vec<NodeId> = closure
            .iter()
            .filter(|t| {
                t.predicate == rdf_type && members.iter().any(|m| t.object == Term::Iri(m.clone()))
            })
            .map(|t| t.subject)
            .collect();
        for x in candidates {
            let all = members.iter().all(|m| {
                closure.iter().any(|t| {
                    t.subject == x && t.predicate == rdf_type && t.object == Term::Iri(m.clone())
                })
            });
            if all {
                frontier.push(Triple::new(x, rdf_type.clone(), restr.clone()));
            }
        }
    }

    // cls-uni: x rdf:type Ci ∧ Ci member of (C1 ∪ … ∪ Cn) → x rdf:type (C1 ∪ … ∪ Cn).
    for t in closure {
        if t.predicate != rdf_type {
            continue;
        }
        for (restr, members) in &union_lists {
            if members.iter().any(|m| t.object == Term::Iri(m.clone())) {
                frontier.push(Triple::new(t.subject, rdf_type.clone(), restr.clone()));
            }
        }
    }

    // cls-oo: ?c owl:oneOf (y1 … yn) → yi rdf:type ?c for every member.
    for t in closure {
        if t.predicate != owl("oneOf") {
            continue;
        }
        let Some(c) = node_term(t.subject) else {
            continue;
        };
        for member in list_terms(&t.object) {
            let Some(member_node) = subject_node(&member) else {
                continue;
            };
            frontier.push(Triple::new(member_node, rdf_type.clone(), c.clone()));
        }
    }

    // scm-int: c owl:intersectionOf (c1 … cn) → c rdfs:subClassOf ci (each member).
    for (restr, members) in &intersection_lists {
        let Some(c_node) = subject_node(restr) else {
            continue;
        };
        for c in members {
            frontier.push(Triple::new(
                c_node,
                rdfs("subClassOf"),
                Term::Iri(c.clone()),
            ));
        }
    }

    // scm-uni: c owl:unionOf (c1 … cn) → ci rdfs:subClassOf c (each member).
    for (restr, members) in &union_lists {
        for c in members {
            frontier.push(Triple::new(
                dict.encode_node(c.as_str()),
                rdfs("subClassOf"),
                restr.clone(),
            ));
        }
    }

    // scm-hv: C1 hasValue i ∧ onProperty p1 ∧ C2 hasValue i ∧ onProperty p2
    // ∧ p1 rdfs:subPropertyOf p2 → C1 rdfs:subClassOf C2.
    for (c1, value) in &has_value {
        let Some(p1) = on_property.iter().find(|(r, _)| r == c1).map(|(_, p)| p) else {
            continue;
        };
        for (c2, value2) in &has_value {
            if value != value2 {
                continue;
            }
            let Some(p2) = on_property.iter().find(|(r, _)| r == c2).map(|(_, p)| p) else {
                continue;
            };
            if p1 != p2
                && subprop.iter().any(|(a, b)| a == p1 && b == p2)
                && let (Some(c1_node), Some(c2_node)) = (subject_node(c1), subject_node(c2))
                && let Some(c2_term) = node_term(c2_node)
            {
                frontier.push(Triple::new(c1_node, rdfs("subClassOf"), c2_term));
            }
        }
    }

    // scm-svf1: C1 some y1 ∧ onProperty p ∧ C2 some y2 ∧ onProperty p ∧ y1 ⊑ y2 → C1 ⊑ C2.
    // scm-svf2: C1 some y ∧ onProperty p1 ∧ C2 some y ∧ onProperty p2 ∧ p1 ⊑ p2 → C1 ⊑ C2.
    for (c1, p) in &on_property {
        let Some(y1) = some_values.iter().find(|(r, _)| r == c1).map(|(_, y)| y) else {
            continue;
        };
        for (c2, p2) in &on_property {
            if p != p2 {
                continue;
            }
            let Some(y2) = some_values.iter().find(|(r, _)| r == c2).map(|(_, y)| y) else {
                continue;
            };
            if y1 != y2
                && subclass.iter().any(|(a, b)| a == y1 && b == y2)
                && let (Some(c1_node), Some(c2_node)) = (subject_node(c1), subject_node(c2))
                && let Some(c2_term) = node_term(c2_node)
            {
                frontier.push(Triple::new(c1_node, rdfs("subClassOf"), c2_term));
            }
        }
    }
    for (c1, y) in &some_values {
        let Some(p1) = on_property.iter().find(|(r, _)| r == c1).map(|(_, p)| p) else {
            continue;
        };
        for (c2, y2) in &some_values {
            if y != y2 {
                continue;
            }
            let Some(p2) = on_property.iter().find(|(r, _)| r == c2).map(|(_, p)| p) else {
                continue;
            };
            if p1 != p2
                && subprop.iter().any(|(a, b)| a == p1 && b == p2)
                && let (Some(c1_node), Some(c2_node)) = (subject_node(c1), subject_node(c2))
                && let Some(c2_term) = node_term(c2_node)
            {
                frontier.push(Triple::new(c1_node, rdfs("subClassOf"), c2_term));
            }
        }
    }

    // scm-avf1: C1 all y1 ∧ onProperty p ∧ C2 all y2 ∧ onProperty p ∧ y1 ⊑ y2 → C1 ⊑ C2.
    for (c1, p) in &on_property {
        let Some(y1) = all_values.iter().find(|(r, _)| r == c1).map(|(_, y)| y) else {
            continue;
        };
        for (c2, p2) in &on_property {
            if p != p2 {
                continue;
            }
            let Some(y2) = all_values.iter().find(|(r, _)| r == c2).map(|(_, y)| y) else {
                continue;
            };
            if y1 != y2
                && subclass.iter().any(|(a, b)| a == y1 && b == y2)
                && let (Some(c1_node), Some(c2_node)) = (subject_node(c1), subject_node(c2))
                && let Some(c2_term) = node_term(c2_node)
            {
                frontier.push(Triple::new(c1_node, rdfs("subClassOf"), c2_term));
            }
        }
    }
    // scm-avf2: C1 all y ∧ onProperty p1 ∧ C2 all y ∧ onProperty p2 ∧ p1 ⊑ p2 → C2 ⊑ C1.
    for (c1, y) in &all_values {
        let Some(p1) = on_property.iter().find(|(r, _)| r == c1).map(|(_, p)| p) else {
            continue;
        };
        for (c2, y2) in &all_values {
            if y != y2 {
                continue;
            }
            let Some(p2) = on_property.iter().find(|(r, _)| r == c2).map(|(_, p)| p) else {
                continue;
            };
            if p1 != p2
                && subprop.iter().any(|(a, b)| a == p1 && b == p2)
                && let (Some(c1_node), Some(c2_node)) = (subject_node(c1), subject_node(c2))
                && let Some(c1_term) = node_term(c1_node)
            {
                frontier.push(Triple::new(c2_node, rdfs("subClassOf"), c1_term));
            }
        }
    }

    // prp-key: x/y share the value of every key property → x owl:sameAs y.
    for (class, keys) in &has_keys {
        let members: Vec<NodeId> = closure
            .iter()
            .filter(|t| t.predicate == rdf_type && &t.object == class)
            .map(|t| t.subject)
            .collect();
        if members.len() < 2 {
            continue;
        }
        // Bucket members by value per key property, then keep pairs that
        // co-occur in a bucket of every key (avoids an O(m^2·k·n^2) scan).
        let buckets: Vec<Vec<(Term, Vec<NodeId>)>> = keys
            .iter()
            .map(|p| {
                let mut by_value: Vec<(Term, Vec<NodeId>)> = Vec::new();
                for &x in &members {
                    for t in closure {
                        if t.subject == x && &t.predicate == p {
                            let value = t.object.clone();
                            match by_value.iter_mut().find(|(v, _)| *v == value) {
                                Some((_, list)) => {
                                    if !list.contains(&x) {
                                        list.push(x);
                                    }
                                }
                                None => by_value.push((value, vec![x])),
                            }
                        }
                    }
                }
                by_value
            })
            .collect();
        let mut pairs: Vec<(NodeId, NodeId)> = Vec::new();
        let mut collect_pairs = |bucket: &[NodeId]| {
            for i in 0..bucket.len() {
                for j in (i + 1)..bucket.len() {
                    let (a, b) = (bucket[i], bucket[j]);
                    let (x, y) = if a < b { (a, b) } else { (b, a) };
                    if !pairs.contains(&(x, y)) {
                        pairs.push((x, y));
                    }
                }
            }
        };
        for (_, list) in &buckets[0] {
            collect_pairs(list);
        }
        for bucket in buckets.iter().skip(1) {
            pairs.retain(|(x, y)| {
                bucket
                    .iter()
                    .any(|(_, list)| list.contains(x) && list.contains(y))
            });
        }
        for (x, y) in pairs {
            if let Some(y_term) = node_term(y) {
                frontier.push(Triple::new(x, owl("sameAs"), y_term));
            }
        }
    }

    // prp-spo2: ?p owl:propertyChainAxiom (?p1 … ?pn) ∧ u1 p1 u2 ∧ … ∧ un pn un+1 → u1 p un+1.
    let property_chains: Vec<(Iri, Vec<Iri>)> = closure
        .iter()
        .filter(|t| t.predicate == owl("propertyChainAxiom"))
        .filter_map(|t| {
            let p = node_iri(t.subject)?;
            let chain = list_members(&t.object);
            (!chain.is_empty()).then_some((p, chain))
        })
        .collect();
    for (p, chain) in &property_chains {
        let mut reachable: Vec<(NodeId, NodeId)> = closure
            .iter()
            .filter(|t| t.predicate == chain[0])
            .filter_map(|t| {
                let o = subject_node(&t.object)?;
                Some((t.subject, o))
            })
            .collect();
        for next in chain.iter().skip(1) {
            let mut step = Vec::new();
            for (start, mid) in &reachable {
                for t in closure {
                    if t.subject == *mid
                        && &t.predicate == next
                        && let Some(o) = subject_node(&t.object)
                    {
                        step.push((*start, o));
                    }
                }
            }
            reachable = step;
        }
        for (u1, un1) in reachable {
            if let Some(un1_term) = node_term(un1) {
                frontier.push(Triple::new(u1, p.clone(), un1_term));
            }
        }
    }

    // owl:sameAs equivalence closure.
    let same_as: Vec<(NodeId, NodeId)> = closure
        .iter()
        .filter_map(|t| {
            if t.predicate != owl("sameAs") {
                return None;
            }
            let o = subject_node(&t.object)?;
            Some((t.subject, o))
        })
        .collect();
    // eq-ref: reflexivity for the terms participating in sameAs components
    // (the full tautology ?x owl:sameAs ?x for every term is not materialized;
    // eq-sym + eq-trans already close the components reflexively).
    let mut reflexive: Vec<NodeId> = Vec::new();
    for (a, b) in &same_as {
        if !reflexive.contains(a) {
            reflexive.push(*a);
        }
        if !reflexive.contains(b) {
            reflexive.push(*b);
        }
    }
    for x in reflexive {
        if let Some(x_term) = node_term(x) {
            frontier.push(Triple::new(x, owl("sameAs"), x_term));
        }
    }
    // eq-sym
    for (a, b) in &same_as {
        if let Some(a_term) = node_term(*a) {
            frontier.push(Triple::new(*b, owl("sameAs"), a_term));
        }
    }
    // eq-trans
    for (a, b) in &same_as {
        for (_, c) in same_as.iter().filter(|(x, _)| x == b) {
            if let Some(c_term) = node_term(*c) {
                frontier.push(Triple::new(*a, owl("sameAs"), c_term));
            }
        }
    }

    // eq-rep-s/o: propagate values across owl:sameAs in subject/object position.
    let mut same_as_map: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for (a, b) in &same_as {
        same_as_map.entry(*a).or_default().push(*b);
    }
    for t in closure {
        if let Some(replacements) = same_as_map.get(&t.subject) {
            for &s2 in replacements {
                frontier.push(Triple::new(s2, t.predicate.clone(), t.object.clone()));
            }
        }
        if let Some(o_node) = subject_node(&t.object)
            && let Some(replacements) = same_as_map.get(&o_node)
        {
            for &o2 in replacements {
                if let Some(o2_term) = node_term(o2) {
                    frontier.push(Triple::new(t.subject, t.predicate.clone(), o2_term));
                }
            }
        }
    }

    // eq-rep-p: predicate replacement via sameAs predicates.
    let mut same_as_iri: HashMap<Iri, Vec<Iri>> = HashMap::new();
    for t in closure {
        if t.predicate == owl("sameAs")
            && let (Some(s), Some(o)) = (node_iri(t.subject), iri_of(&t.object))
        {
            same_as_iri.entry(s).or_default().push(o);
        }
    }
    for t in closure {
        if let Some(replacements) = same_as_iri.get(&t.predicate) {
            for p2 in replacements {
                frontier.push(Triple::new(t.subject, p2.clone(), t.object.clone()));
            }
        }
    }

    // Consistency rules: conclusions are ⊥ and surface as `inconsistent`.
    // Indexes include same-iteration derivations (`frontier`) so chain-triggered
    // contradictions are detected even when `max_iterations == 1`.
    let nothing = owl("Nothing");
    let all: Vec<&Triple> = closure.iter().chain(frontier.iter()).collect();
    let subclass_all: Vec<(Iri, Iri)> = all
        .iter()
        .filter_map(|t| {
            if t.predicate != rdfs("subClassOf") {
                return None;
            }
            let (s, o) = (node_iri(t.subject)?, iri_of(&t.object)?);
            Some((s, o))
        })
        .collect();
    let disjoint: Vec<(Iri, Iri)> = all
        .iter()
        .filter_map(|t| {
            if t.predicate != owl("disjointWith") {
                return None;
            }
            let (s, o) = (node_iri(t.subject)?, iri_of(&t.object)?);
            Some((s, o))
        })
        .collect();
    let complement: Vec<(Iri, Iri)> = all
        .iter()
        .filter_map(|t| {
            if t.predicate != owl("complementOf") {
                return None;
            }
            let (s, o) = (node_iri(t.subject)?, iri_of(&t.object)?);
            Some((s, o))
        })
        .collect();
    let same_as_all: Vec<(NodeId, NodeId)> = all
        .iter()
        .filter_map(|t| {
            if t.predicate != owl("sameAs") {
                return None;
            }
            let o = subject_node(&t.object)?;
            Some((t.subject, o))
        })
        .collect();
    let different_from = owl("differentFrom");
    for t in &all {
        if t.predicate != rdf_type {
            continue;
        }
        let Some(c1) = iri_of(&t.object) else {
            continue;
        };
        // cls-nothing1: x rdf:type owl:Nothing → ⊥.
        if c1 == nothing {
            *inconsistent = true;
        }
        // cls-nothing2: ?c rdfs:subClassOf owl:Nothing ∧ x rdf:type ?c → ⊥.
        if subclass_all.iter().any(|(c, d)| *d == nothing && *c == c1) {
            *inconsistent = true;
        }
        // cax-dw: x rdf:type ?c1 ∧ x rdf:type ?c2 ∧ ?c1 owl:disjointWith ?c2 → ⊥.
        let typed = |c: &Iri| -> bool {
            all.iter().any(|u| {
                u.subject == t.subject
                    && u.predicate == rdf_type
                    && iri_of(&u.object).as_ref() == Some(c)
            })
        };
        if disjoint
            .iter()
            .any(|(a, b)| (a == &c1 && typed(b)) || (b == &c1 && typed(a)))
        {
            *inconsistent = true;
        }
        // cls-com: x rdf:type ?c1 ∧ x rdf:type ?c2 ∧ ?c1 owl:complementOf ?c2 → ⊥.
        if complement
            .iter()
            .any(|(a, b)| (a == &c1 && typed(b)) || (b == &c1 && typed(a)))
        {
            *inconsistent = true;
        }
    }
    for t in &all {
        if t.predicate != different_from {
            continue;
        }
        let Some(obj_node) = subject_node(&t.object) else {
            continue;
        };
        // eq-diff1: x owl:sameAs y ∧ x owl:differentFrom y → ⊥. The reflexive
        // corollary x owl:differentFrom x is detected by the same check (eq-refl).
        if t.subject == obj_node {
            *inconsistent = true;
        }
        if same_as_all
            .iter()
            .any(|(a, b)| *a == t.subject && *b == obj_node)
        {
            *inconsistent = true;
        }
    }

    // prp-irp: x ?p x ∧ ?p rdf:type owl:IrreflexiveProperty → ⊥.
    let irreflexive: Vec<Iri> = all
        .iter()
        .filter_map(|t| {
            (t.predicate == rdf_type && t.object == Term::Iri(owl("IrreflexiveProperty")))
                .then(|| node_iri(t.subject))
                .flatten()
        })
        .collect();
    if irreflexive.iter().any(|p| {
        all.iter()
            .any(|t| &t.predicate == p && subject_node(&t.object) == Some(t.subject))
    }) {
        *inconsistent = true;
    }

    // prp-asyp: ?p rdf:type owl:AsymmetricProperty ∧ x p y ∧ y p x → ⊥.
    let asymmetric: Vec<Iri> = all
        .iter()
        .filter_map(|t| {
            (t.predicate == rdf_type && t.object == Term::Iri(owl("AsymmetricProperty")))
                .then(|| node_iri(t.subject))
                .flatten()
        })
        .collect();
    let mut asymmetric_violated = false;
    'asym: for p in &asymmetric {
        let mut pairs: Vec<(NodeId, NodeId)> = Vec::new();
        for t in all.iter().filter(|t| t.predicate.as_str() == p.as_str()) {
            let Some(o) = subject_node(&t.object) else {
                continue;
            };
            let pair = (t.subject, o);
            if pair.0 == pair.1 {
                continue;
            }
            if pairs.contains(&(pair.1, pair.0)) {
                asymmetric_violated = true;
                break 'asym;
            }
            if !pairs.contains(&pair) {
                pairs.push(pair);
            }
        }
    }
    if asymmetric_violated {
        *inconsistent = true;
    }

    // prp-pdw: ?p1 owl:propertyDisjointWith ?p2 ∧ x p1 y ∧ x p2 y → ⊥.
    let property_disjoint: Vec<(Iri, Iri)> = all
        .iter()
        .filter_map(|t| {
            if t.predicate != owl("propertyDisjointWith") {
                return None;
            }
            let (s, o) = (node_iri(t.subject)?, iri_of(&t.object)?);
            Some((s, o))
        })
        .collect();
    if property_disjoint.iter().any(|(p1, p2)| {
        all.iter().any(|t| {
            &t.predicate == p1
                && all.iter().any(|u| {
                    u.subject == t.subject
                        && u.predicate.as_str() == p2.as_str()
                        && u.object == t.object
                })
        })
    }) {
        *inconsistent = true;
    }

    // cls-maxc1: (p max 0) ∧ u rdf:type (p max 0) ∧ u p y → ⊥.
    let mut maxc0_violated = false;
    'maxc0: for (restr, p) in &max_cardinality_zero {
        for t in all.iter().filter(|t| t.predicate.as_str() == p.as_str()) {
            let typed_restr = all
                .iter()
                .any(|u| u.subject == t.subject && u.predicate == rdf_type && u.object == *restr);
            if typed_restr {
                maxc0_violated = true;
                break 'maxc0;
            }
        }
    }
    if maxc0_violated {
        *inconsistent = true;
    }

    // cls-maxqc1: (p max 0 c) ∧ u type (p max 0 c) ∧ u p y ∧ y type c → ⊥.
    // cls-maxqc2: same with c = owl:Thing (every value qualifies).
    let mut maxqc0_violated = false;
    'maxqc0: for (restr, p, c) in &max_qualified_zero {
        for t in all.iter().filter(|t| t.predicate.as_str() == p.as_str()) {
            let typed_restr = all
                .iter()
                .any(|u| u.subject == t.subject && u.predicate == rdf_type && u.object == *restr);
            if !typed_restr {
                continue;
            }
            if *c == owl("Thing") {
                maxqc0_violated = true;
                break 'maxqc0;
            }
            let Some(y) = subject_node(&t.object) else {
                continue;
            };
            if all.iter().any(|u| {
                u.subject == y && u.predicate == rdf_type && u.object == Term::Iri(c.clone())
            }) {
                maxqc0_violated = true;
                break 'maxqc0;
            }
        }
    }
    if maxqc0_violated {
        *inconsistent = true;
    }

    // prp-adp: ?x rdf:type owl:AllDisjointProperties ∧ ?x owl:members (?p1 … ?pn)
    // ∧ u pi v ∧ u pj v → ⊥.
    let all_disjoint_properties: Vec<Vec<Iri>> = all
        .iter()
        .filter(|t| t.predicate == owl("members"))
        .filter_map(|t| {
            let typed = all.iter().any(|u| {
                u.subject == t.subject
                    && u.predicate == rdf_type
                    && u.object == Term::Iri(owl("AllDisjointProperties"))
            });
            typed.then(|| list_members(&t.object))
        })
        .collect();
    for properties in &all_disjoint_properties {
        let mut violated = false;
        'outer: for t in all.iter().filter(|t| properties.contains(&t.predicate)) {
            for pj in properties {
                if pj.as_str() != t.predicate.as_str()
                    && all.iter().any(|u| {
                        u.subject == t.subject
                            && u.predicate.as_str() == pj.as_str()
                            && u.object == t.object
                    })
                {
                    violated = true;
                    break 'outer;
                }
            }
        }
        if violated {
            *inconsistent = true;
        }
    }

    // prp-npa1 / prp-npa2: NegativePropertyAssertion — asserted fact violates it.
    for t in &all {
        if t.predicate != owl("sourceIndividual") {
            continue;
        }
        let Some(i) = subject_node(&t.object) else {
            continue;
        };
        let properties: Vec<Iri> = all
            .iter()
            .filter(|u| u.subject == t.subject && u.predicate == owl("assertionProperty"))
            .filter_map(|u| iri_of(&u.object))
            .collect();
        for p in &properties {
            let targets: Vec<Term> = all
                .iter()
                .filter(|u| {
                    u.subject == t.subject
                        && (u.predicate == owl("targetIndividual")
                            || u.predicate == owl("targetValue"))
                })
                .map(|u| u.object.clone())
                .collect();
            for target in &targets {
                if all.iter().any(|u| {
                    u.subject == i && u.predicate.as_str() == p.as_str() && &u.object == target
                }) {
                    *inconsistent = true;
                }
            }
        }
    }

    // dt-not-type: a literal whose lexical form lies outside its datatype's
    // value space is inconsistent.
    if all.iter().any(|t| {
        if let Term::Literal(value) = &t.object {
            !literal_lexically_valid(value)
        } else {
            false
        }
    }) {
        *inconsistent = true;
    }

    // cax-adc: ?x rdf:type owl:AllDisjointClasses ∧ ?x owl:members (?c1 … ?cn)
    // ∧ z rdf:type ?ci ∧ z rdf:type ?cj → ⊥.
    let all_disjoint_classes: Vec<Vec<Iri>> = all
        .iter()
        .filter(|t| t.predicate == owl("members"))
        .filter_map(|t| {
            let typed = all.iter().any(|u| {
                u.subject == t.subject
                    && u.predicate == rdf_type
                    && u.object == Term::Iri(owl("AllDisjointClasses"))
            });
            typed.then(|| list_members(&t.object))
        })
        .collect();
    for classes in &all_disjoint_classes {
        let mut by_subject: HashMap<NodeId, Vec<Iri>> = HashMap::new();
        for t in &all {
            if t.predicate == rdf_type
                && let Some(c) = iri_of(&t.object)
                && classes.contains(&c)
            {
                by_subject.entry(t.subject).or_default().push(c);
            }
        }
        if by_subject.values().any(|typed| {
            let mut seen = Vec::new();
            for c in typed {
                if !seen.contains(c) {
                    seen.push(c.clone());
                }
            }
            seen.len() >= 2
        }) {
            *inconsistent = true;
        }
    }

    // eq-diff2/3: ?x rdf:type owl:AllDifferent ∧ ?x owl:members / owl:distinctMembers
    // (?z1 … ?zn) ∧ ?zi owl:sameAs ?zj → ⊥.
    for members_predicate in [owl("members"), owl("distinctMembers")] {
        let all_different_lists: Vec<Vec<Term>> = all
            .iter()
            .filter(|t| t.predicate == members_predicate)
            .filter_map(|t| {
                let typed = all.iter().any(|u| {
                    u.subject == t.subject
                        && u.predicate == rdf_type
                        && u.object == Term::Iri(owl("AllDifferent"))
                });
                typed.then(|| list_terms(&t.object))
            })
            .collect();
        for members in &all_different_lists {
            for i in 0..members.len() {
                for j in (i + 1)..members.len() {
                    let (Some(zi), Some(zj)) =
                        (subject_node(&members[i]), subject_node(&members[j]))
                    else {
                        continue;
                    };
                    if same_as_all
                        .iter()
                        .any(|(a, b)| (*a == zi && *b == zj) || (*a == zj && *b == zi))
                    {
                        *inconsistent = true;
                    }
                }
            }
        }
    }
}

fn absorb_new(closure: &mut Vec<Triple>, frontier: Vec<Triple>) -> usize {
    let before = closure.len();
    for triple in frontier {
        if !closure.contains(&triple) {
            closure.push(triple);
        }
    }
    closure.len() - before
}

pub fn status() -> &'static str {
    "infrastructure"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{InferenceMode, ReasoningTask};
    use ontolith_core::domain::LiteralValue;
    use ontolith_storage::infrastructure::InMemoryDictionary;

    fn task(mode: InferenceMode) -> ReasoningTask {
        ReasoningTask {
            plan_id: None,
            mode,
            max_iterations: 16,
            max_elapsed_ms: None,
        }
    }

    fn t(s: &str, p: &str, o: &str, dict: &InMemoryDictionary) -> Triple {
        Triple::new(dict.encode_node(s), Iri::new(p), Term::Iri(Iri::new(o)))
    }

    fn tb(s: &str, p: &str, bnode: &str, dict: &InMemoryDictionary) -> Triple {
        Triple::new(
            dict.encode_node(s),
            Iri::new(p),
            Term::BlankNode(dict.encode_node(bnode)),
        )
    }

    fn tl(s: &str, p: &str, o: LiteralValue, dict: &InMemoryDictionary) -> Triple {
        Triple::new(dict.encode_node(s), Iri::new(p), Term::Literal(o))
    }

    /// Two-triple-per-node RDF list encoding: `start` → first/member + rest/nil.
    fn rdf_list(dict: &InMemoryDictionary, start: &str, members: &[&str]) -> Vec<Triple> {
        let rdf_first = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
        let rdf_rest = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
        let rdf_nil = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
        let prefix = start.trim_start_matches("_:");
        let mut out = Vec::new();
        for (i, m) in members.iter().enumerate() {
            let node = if i == 0 {
                start.to_string()
            } else {
                format!("_:{prefix}{i}")
            };
            out.push(t(&node, rdf_first, m, dict));
            let rest = if i + 1 == members.len() {
                rdf_nil.to_string()
            } else {
                format!("_:{prefix}{}", i + 1)
            };
            out.push(t(&node, rdf_rest, &rest, dict));
        }
        out
    }

    fn task_max_iterations(n: u32) -> ReasoningTask {
        ReasoningTask {
            plan_id: None,
            mode: InferenceMode::ForwardChaining,
            max_iterations: n,
            max_elapsed_ms: None,
        }
    }

    #[test]
    fn subclass_transitivity_infers_ancestor() {
        let dict = InMemoryDictionary::new();
        let input = vec![
            t(
                "urn:A",
                "http://www.w3.org/2000/01/rdf-schema#subClassOf",
                "urn:B",
                &dict,
            ),
            t(
                "urn:B",
                "http://www.w3.org/2000/01/rdf-schema#subClassOf",
                "urn:C",
                &dict,
            ),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert_eq!(outcome.report.inferred_triples, 1);
        assert!(outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:A")
                && tr.predicate.as_str() == "http://www.w3.org/2000/01/rdf-schema#subClassOf"
                && tr.object == Term::Iri(Iri::new("urn:C"))
        }));
    }

    #[test]
    fn domain_and_range_emit_type_triples() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let input = vec![
            t("urn:knows", &format!("{rdfs}domain"), "urn:Person", &dict),
            t("urn:knows", &format!("{rdfs}range"), "urn:Person", &dict),
            t("urn:alice", "urn:knows", "urn:bob", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let alice_type = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:alice")
                && tr.predicate.as_str() == rdf_type
                && tr.object == Term::Iri(Iri::new("urn:Person"))
        });
        let bob_type = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:bob")
                && tr.predicate.as_str() == rdf_type
                && tr.object == Term::Iri(Iri::new("urn:Person"))
        });
        assert!(alice_type, "expected alice rdf:type Person");
        assert!(bob_type, "expected bob rdf:type Person");
    }

    #[test]
    fn subproperty_and_inverse_apply_to_data() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let owl = "http://www.w3.org/2002/07/owl#";
        let input = vec![
            t(
                "urn:author",
                &format!("{rdfs}subPropertyOf"),
                "urn:creator",
                &dict,
            ),
            t(
                "urn:knows",
                &format!("{owl}inverseOf"),
                "urn:knownBy",
                &dict,
            ),
            t("urn:alice", "urn:author", "urn:book1", &dict),
            t("urn:alice", "urn:knows", "urn:bob", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        let creator = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:alice")
                && tr.predicate.as_str() == "urn:creator"
                && tr.object == Term::Iri(Iri::new("urn:book1"))
        });
        let known_by = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:bob")
                && tr.predicate.as_str() == "urn:knownBy"
                && tr.object == Term::Iri(Iri::new("urn:alice"))
        });
        assert!(creator, "expected alice creator book1");
        assert!(known_by, "expected bob knownBy alice");
    }

    #[test]
    fn mode_off_returns_input_unchanged() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let input = vec![
            t("urn:A", &format!("{rdfs}subClassOf"), "urn:B", &dict),
            t("urn:B", &format!("{rdfs}subClassOf"), "urn:C", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::Off), &input)
            .expect("materialize");
        assert_eq!(outcome.triples, input);
        assert_eq!(outcome.report.inferred_triples, 0);
    }

    #[test]
    fn subclass_application_infers_types() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t(
                "urn:Person",
                &format!("{rdfs}subClassOf"),
                "urn:Agent",
                &dict,
            ),
            t("urn:alice", rdf_type, "urn:Person", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:alice")
                && tr.predicate.as_str() == rdf_type
                && tr.object == Term::Iri(Iri::new("urn:Agent"))
        }));
    }

    #[test]
    fn symmetric_property_infers_reverse() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t(
                "urn:knows",
                rdf_type,
                &format!("{owl}SymmetricProperty"),
                &dict,
            ),
            t("urn:alice", "urn:knows", "urn:bob", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:bob")
                && tr.predicate.as_str() == "urn:knows"
                && tr.object == Term::Iri(Iri::new("urn:alice"))
        }));
    }

    #[test]
    fn transitive_property_infers_chain() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t(
                "urn:ancestorOf",
                rdf_type,
                &format!("{owl}TransitiveProperty"),
                &dict,
            ),
            t("urn:alice", "urn:ancestorOf", "urn:bob", &dict),
            t("urn:bob", "urn:ancestorOf", "urn:carol", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:alice")
                && tr.predicate.as_str() == "urn:ancestorOf"
                && tr.object == Term::Iri(Iri::new("urn:carol"))
        }));
    }

    #[test]
    fn inverse_reverse_direction() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let input = vec![
            t(
                "urn:knows",
                &format!("{owl}inverseOf"),
                "urn:knownBy",
                &dict,
            ),
            t("urn:alice", "urn:knows", "urn:bob", &dict),
            t("urn:carol", "urn:knownBy", "urn:dave", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        let known_by = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:bob")
                && tr.predicate.as_str() == "urn:knownBy"
                && tr.object == Term::Iri(Iri::new("urn:alice"))
        });
        let knows = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:dave")
                && tr.predicate.as_str() == "urn:knows"
                && tr.object == Term::Iri(Iri::new("urn:carol"))
        });
        assert!(known_by, "expected bob knownBy alice (prp-inv1)");
        assert!(knows, "expected dave knows carol (prp-inv2)");
    }

    #[test]
    fn supported_rules_covers_extended_set() {
        let reasoner = ForwardChainReasoner::new();
        let names: Vec<&str> = reasoner
            .supported_rules()
            .iter()
            .map(|r| r.as_str())
            .collect();
        // The complete W3C OWL 2 RL rule table (Tables 4-9) plus the RDFS
        // rules not subsumed by RL (rdf11-mt Table 1) and cls-nothing3.
        let expected = [
            // Equality.
            "eq-ref",
            "eq-sym",
            "eq-trans",
            "eq-rep-s",
            "eq-rep-p",
            "eq-rep-o",
            "eq-diff1",
            "eq-diff2",
            "eq-diff3",
            // Property axioms.
            "prp-ap",
            "prp-dom",
            "prp-rng",
            "prp-fp",
            "prp-ifp",
            "prp-irp",
            "prp-symp",
            "prp-asyp",
            "prp-trp",
            "prp-spo1",
            "prp-spo2",
            "prp-eqp1",
            "prp-eqp2",
            "prp-pdw",
            "prp-adp",
            "prp-inv1",
            "prp-inv2",
            "prp-key",
            "prp-npa1",
            "prp-npa2",
            // Class expressions.
            "cls-thing",
            "cls-nothing1",
            "cls-nothing2",
            "cls-nothing3",
            "cls-int1",
            "cls-int2",
            "cls-uni",
            "cls-com",
            "cls-svf1",
            "cls-svf2",
            "cls-avf",
            "cls-hv1",
            "cls-hv2",
            "cls-maxc1",
            "cls-maxc2",
            "cls-maxqc1",
            "cls-maxqc2",
            "cls-maxqc3",
            "cls-maxqc4",
            "cls-oo",
            // Class axioms.
            "cax-sco",
            "cax-eqc1",
            "cax-eqc2",
            "cax-dw",
            "cax-adc",
            // Datatypes.
            "dt-type1",
            "dt-type2",
            "dt-not-type",
            "dt-eq",
            "dt-diff",
            // Schema vocabulary.
            "scm-cls",
            "scm-sco",
            "scm-eqc1",
            "scm-eqc2",
            "scm-op",
            "scm-dp",
            "scm-spo",
            "scm-eqp1",
            "scm-eqp2",
            "scm-dom1",
            "scm-dom2",
            "scm-rng1",
            "scm-rng2",
            "scm-hv",
            "scm-svf1",
            "scm-svf2",
            "scm-avf1",
            "scm-avf2",
            "scm-int",
            "scm-uni",
            // RDFS rules not subsumed by OWL 2 RL.
            "rdfs1",
            "rdfs4a",
            "rdfs4b",
            "rdfs6",
            "rdfs8",
            "rdfs10",
            "rdfs12",
            "rdfs13",
        ];
        assert_eq!(
            names.len(),
            expected.len(),
            "supported rule set must be exactly the complete list"
        );
        for expected in expected {
            assert!(names.contains(&expected), "missing rule {expected}");
        }
    }

    #[test]
    fn intersection_of_types_members_both_directions() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let rdf_nil = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
        let input = vec![
            tb("_:r", &format!("{owl}intersectionOf"), "_:l1", &dict),
            t(
                "_:l1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#first",
                "urn:A",
                &dict,
            ),
            tb(
                "_:l1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest",
                "_:l2",
                &dict,
            ),
            t(
                "_:l2",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#first",
                "urn:B",
                &dict,
            ),
            t(
                "_:l2",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest",
                rdf_nil,
                &dict,
            ),
            tb("urn:alice", rdf_type, "_:r", &dict),
            t("urn:bob", rdf_type, "urn:A", &dict),
            t("urn:bob", rdf_type, "urn:B", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        let alice_a = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:alice")
                && tr.predicate.as_str() == rdf_type
                && tr.object == Term::Iri(Iri::new("urn:A"))
        });
        let alice_b = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:alice")
                && tr.predicate.as_str() == rdf_type
                && tr.object == Term::Iri(Iri::new("urn:B"))
        });
        let bob_restr = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:bob")
                && tr.predicate.as_str() == rdf_type
                && tr.object == Term::BlankNode(dict.encode_node("_:r"))
        });
        assert!(alice_a, "expected alice type A (cls-int1)");
        assert!(alice_b, "expected alice type B (cls-int1)");
        assert!(bob_restr, "expected bob type intersection (cls-int2)");
    }

    #[test]
    fn union_of_types_restriction() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let rdf_nil = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
        let input = vec![
            tb("_:u", &format!("{owl}unionOf"), "_:l1", &dict),
            t(
                "_:l1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#first",
                "urn:A",
                &dict,
            ),
            tb(
                "_:l1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest",
                "_:l2",
                &dict,
            ),
            t(
                "_:l2",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#first",
                "urn:B",
                &dict,
            ),
            t(
                "_:l2",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest",
                rdf_nil,
                &dict,
            ),
            t("urn:carol", rdf_type, "urn:A", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:carol")
                && tr.predicate.as_str() == rdf_type
                && tr.object == Term::BlankNode(dict.encode_node("_:u"))
        }));
    }

    #[test]
    fn same_as_equivalence_closure() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let same_as = format!("{owl}sameAs");
        let input = vec![
            t("urn:a", &same_as, "urn:b", &dict),
            t("urn:b", &same_as, "urn:c", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        let a_c = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:a")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:c"))
        });
        let b_a = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:b")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:a"))
        });
        assert!(a_c, "expected a sameAs c (eq-trans)");
        assert!(b_a, "expected b sameAs a (eq-sym)");
    }

    #[test]
    fn some_values_from_restriction_types_values() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("_:r", &format!("{owl}onProperty"), "urn:knows", &dict),
            t("_:r", &format!("{owl}someValuesFrom"), "urn:Person", &dict),
            tb("urn:alice", rdf_type, "_:r", &dict),
            t("urn:alice", "urn:knows", "urn:bob", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:bob")
                && tr.predicate.as_str() == rdf_type
                && tr.object == Term::Iri(Iri::new("urn:Person"))
        }));
    }

    #[test]
    fn some_values_from_backward_typing() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("_:r", &format!("{owl}onProperty"), "urn:knows", &dict),
            t("_:r", &format!("{owl}someValuesFrom"), "urn:Person", &dict),
            t("urn:alice", "urn:knows", "urn:bob", &dict),
            t("urn:bob", rdf_type, "urn:Person", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:alice")
                && tr.predicate.as_str() == rdf_type
                && tr.object == Term::BlankNode(dict.encode_node("_:r"))
        }));
    }

    #[test]
    fn all_values_from_restriction_types_values() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("_:r", &format!("{owl}onProperty"), "urn:knows", &dict),
            t("_:r", &format!("{owl}allValuesFrom"), "urn:Person", &dict),
            tb("urn:alice", rdf_type, "_:r", &dict),
            t("urn:alice", "urn:knows", "urn:bob", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:bob")
                && tr.predicate.as_str() == rdf_type
                && tr.object == Term::Iri(Iri::new("urn:Person"))
        }));
    }

    #[test]
    fn has_value_infers_triple_from_typing() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("_:r", &format!("{owl}onProperty"), "urn:knows", &dict),
            t("_:r", &format!("{owl}hasValue"), "urn:bob", &dict),
            tb("urn:alice", rdf_type, "_:r", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            outcome.triples.iter().any(|tr| {
                tr.subject == dict.encode_node("urn:alice")
                    && tr.predicate.as_str() == "urn:knows"
                    && tr.object == Term::Iri(Iri::new("urn:bob"))
            }),
            "expected alice knows bob (cls-hv1)"
        );
    }

    #[test]
    fn has_value_typing_infers_restriction() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("_:r", &format!("{owl}onProperty"), "urn:age", &dict),
            tl(
                "_:r",
                &format!("{owl}hasValue"),
                LiteralValue::Integer(42),
                &dict,
            ),
            tl("urn:alice", "urn:age", LiteralValue::Integer(42), &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            outcome.triples.iter().any(|tr| {
                tr.subject == dict.encode_node("urn:alice")
                    && tr.predicate.as_str() == rdf_type
                    && tr.object == Term::BlankNode(dict.encode_node("_:r"))
            }),
            "expected alice rdf:type _:r from literal hasValue match (cls-hv2)"
        );
    }

    #[test]
    fn property_chain_infers_chained_triple() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let mut input = vec![
            t(
                "urn:uncleOf",
                &format!("{owl}propertyChainAxiom"),
                "_:u",
                &dict,
            ),
            t(
                "urn:ancestorOf",
                &format!("{owl}propertyChainAxiom"),
                "_:a",
                &dict,
            ),
        ];
        input.extend(rdf_list(&dict, "_:u", &["urn:siblingOf", "urn:parentOf"]));
        input.extend(rdf_list(
            &dict,
            "_:a",
            &["urn:parentOf", "urn:parentOf", "urn:parentOf"],
        ));
        input.push(t("urn:alice", "urn:parentOf", "urn:bob", &dict));
        input.push(t("urn:alice", "urn:siblingOf", "urn:bob", &dict));
        input.push(t("urn:bob", "urn:parentOf", "urn:carol", &dict));
        input.push(t("urn:carol", "urn:parentOf", "urn:dave", &dict));
        input.push(t("urn:dave", "urn:parentOf", "urn:erin", &dict));
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            outcome.triples.iter().any(|tr| {
                tr.subject == dict.encode_node("urn:alice")
                    && tr.predicate.as_str() == "urn:uncleOf"
                    && tr.object == Term::Iri(Iri::new("urn:carol"))
            }),
            "expected alice uncleOf carol (2-member prp-spo2)"
        );
        assert!(
            outcome.triples.iter().any(|tr| {
                tr.subject == dict.encode_node("urn:alice")
                    && tr.predicate.as_str() == "urn:ancestorOf"
                    && tr.object == Term::Iri(Iri::new("urn:dave"))
            }),
            "expected alice ancestorOf dave (3-member prp-spo2)"
        );
    }

    #[test]
    fn irreflexive_property_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let reasoner = ForwardChainReasoner::new();
        let conflicting = vec![
            t(
                "urn:p",
                rdf_type,
                &format!("{owl}IrreflexiveProperty"),
                &dict,
            ),
            t("urn:x", "urn:p", "urn:x", &dict),
        ];
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &conflicting)
            .expect("materialize");
        assert!(outcome.report.inconsistent, "expected prp-irp ⊥ (x p x)");
        let fine = vec![
            t(
                "urn:p",
                rdf_type,
                &format!("{owl}IrreflexiveProperty"),
                &dict,
            ),
            t("urn:x", "urn:p", "urn:y", &dict),
        ];
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &fine)
            .expect("materialize");
        assert!(
            !outcome.report.inconsistent,
            "x p y must not flag prp-irp ⊥"
        );
    }

    #[test]
    fn all_disjoint_classes_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let reasoner = ForwardChainReasoner::new();
        let mut conflicting = vec![
            t(
                "_:adc",
                rdf_type,
                &format!("{owl}AllDisjointClasses"),
                &dict,
            ),
            t("_:adc", &format!("{owl}members"), "_:l", &dict),
            t("urn:z", rdf_type, "urn:A", &dict),
            t("urn:z", rdf_type, "urn:B", &dict),
        ];
        conflicting.extend(rdf_list(&dict, "_:l", &["urn:A", "urn:B"]));
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &conflicting)
            .expect("materialize");
        assert!(
            outcome.report.inconsistent,
            "expected cax-adc ⊥ (z typed into two disjoint classes)"
        );
        let mut fine = vec![
            t(
                "_:adc",
                rdf_type,
                &format!("{owl}AllDisjointClasses"),
                &dict,
            ),
            t("_:adc", &format!("{owl}members"), "_:l", &dict),
            t("urn:z", rdf_type, "urn:A", &dict),
            t("urn:w", rdf_type, "urn:B", &dict),
        ];
        fine.extend(rdf_list(&dict, "_:l", &["urn:A", "urn:B"]));
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &fine)
            .expect("materialize");
        assert!(
            !outcome.report.inconsistent,
            "disjoint classes with distinct members must stay consistent"
        );
    }

    #[test]
    fn all_different_members_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let reasoner = ForwardChainReasoner::new();
        let mut conflicting = vec![
            t("_:ad", rdf_type, &format!("{owl}AllDifferent"), &dict),
            t("_:ad", &format!("{owl}members"), "_:l", &dict),
            t("urn:a", &format!("{owl}sameAs"), "urn:b", &dict),
        ];
        conflicting.extend(rdf_list(&dict, "_:l", &["urn:a", "urn:b"]));
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &conflicting)
            .expect("materialize");
        assert!(
            outcome.report.inconsistent,
            "expected eq-diff2 ⊥ (owl:members pair sameAs)"
        );
        let mut fine = vec![
            t("_:ad", rdf_type, &format!("{owl}AllDifferent"), &dict),
            t("_:ad", &format!("{owl}members"), "_:l", &dict),
        ];
        fine.extend(rdf_list(&dict, "_:l", &["urn:a", "urn:b"]));
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &fine)
            .expect("materialize");
        assert!(
            !outcome.report.inconsistent,
            "AllDifferent without sameAs pair must stay consistent"
        );
    }

    #[test]
    fn all_different_distinct_members_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let mut input = vec![
            t("_:ad", rdf_type, &format!("{owl}AllDifferent"), &dict),
            t("_:ad", &format!("{owl}distinctMembers"), "_:l", &dict),
            t("urn:a", &format!("{owl}sameAs"), "urn:b", &dict),
        ];
        input.extend(rdf_list(&dict, "_:l", &["urn:a", "urn:b"]));
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            outcome.report.inconsistent,
            "expected eq-diff3 ⊥ (owl:distinctMembers pair sameAs)"
        );
    }

    #[test]
    fn wall_clock_budget_guards_materialization() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let input = vec![
            t("urn:A", &format!("{rdfs}subClassOf"), "urn:B", &dict),
            t("urn:B", &format!("{rdfs}subClassOf"), "urn:C", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();

        let exhausted = ReasoningTask {
            plan_id: None,
            mode: InferenceMode::ForwardChaining,
            max_iterations: 16,
            max_elapsed_ms: Some(0),
        };
        let outcome = reasoner
            .materialize(&dict, &exhausted, &input)
            .expect("materialize");
        assert!(outcome.report.timed_out, "expected early stop");
        assert_eq!(outcome.report.inferred_triples, 0);

        let ample = ReasoningTask {
            plan_id: None,
            mode: InferenceMode::ForwardChaining,
            max_iterations: 16,
            max_elapsed_ms: Some(60_000),
        };
        let outcome = reasoner
            .materialize(&dict, &ample, &input)
            .expect("materialize");
        assert!(!outcome.report.timed_out);
        assert!(outcome.report.inferred_triples >= 1);
    }

    #[test]
    fn has_key_properties_infer_same_as() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let rdf_nil = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
        let input = vec![
            t("urn:Person", &format!("{owl}hasKey"), "_:l1", &dict),
            t(
                "_:l1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#first",
                "urn:ssn",
                &dict,
            ),
            tb(
                "_:l1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest",
                "_:l2",
                &dict,
            ),
            t(
                "_:l2",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#first",
                "urn:email",
                &dict,
            ),
            t(
                "_:l2",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest",
                rdf_nil,
                &dict,
            ),
            t("urn:alice", rdf_type, "urn:Person", &dict),
            t("urn:bob", rdf_type, "urn:Person", &dict),
            t("urn:carol", rdf_type, "urn:Person", &dict),
            t("urn:alice", "urn:ssn", "urn:v1", &dict),
            t("urn:alice", "urn:email", "urn:e1", &dict),
            t("urn:bob", "urn:ssn", "urn:v1", &dict),
            t("urn:bob", "urn:email", "urn:e1", &dict),
            t("urn:carol", "urn:ssn", "urn:v1", &dict),
            t("urn:carol", "urn:email", "urn:e2", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        let same_as = format!("{owl}sameAs");
        let alice_bob = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:alice")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:bob"))
        });
        let bob_alice = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:bob")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:alice"))
        });
        let carol_alice = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:carol")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:alice"))
        });
        assert!(alice_bob, "expected alice sameAs bob (prp-key)");
        assert!(bob_alice, "expected bob sameAs alice (eq-sym)");
        assert!(!carol_alice, "carol differs on one key property");
    }

    #[test]
    fn disjoint_classes_mark_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("urn:Cat", &format!("{owl}disjointWith"), "urn:Dog", &dict),
            t("urn:rex", rdf_type, "urn:Cat", &dict),
            t("urn:rex", rdf_type, "urn:Dog", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.report.inconsistent, "expected cax-dw ⊥");
    }

    #[test]
    fn nothing_typing_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("urn:x", rdf_type, &format!("{owl}Nothing"), &dict),
            t(
                "urn:Empty",
                &format!("{rdfs}subClassOf"),
                &format!("{owl}Nothing"),
                &dict,
            ),
            t("urn:y", rdf_type, "urn:Empty", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.report.inconsistent, "expected cls-nothing1/2 ⊥");
    }

    #[test]
    fn different_from_conflict_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let different_from = format!("{owl}differentFrom");
        let same_as = format!("{owl}sameAs");
        let input = vec![
            t("urn:a", &different_from, "urn:a", &dict),
            t("urn:b", &same_as, "urn:c", &dict),
            t("urn:b", &different_from, "urn:c", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.report.inconsistent, "expected eq-diff1 ⊥");
    }

    #[test]
    fn consistent_input_not_marked_inconsistent() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("urn:A", &format!("{rdfs}subClassOf"), "urn:B", &dict),
            t("urn:B", &format!("{rdfs}subClassOf"), "urn:C", &dict),
            t("urn:x", rdf_type, "urn:A", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            !outcome.report.inconsistent,
            "consistent input must not flag ⊥"
        );
        assert!(outcome.report.inferred_triples >= 2);
    }

    #[test]
    fn has_key_single_property_infers_same_as() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let rdf_nil = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
        let input = vec![
            t("urn:Person", &format!("{owl}hasKey"), "_:l1", &dict),
            t(
                "_:l1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#first",
                "urn:ssn",
                &dict,
            ),
            t(
                "_:l1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest",
                rdf_nil,
                &dict,
            ),
            t("urn:alice", rdf_type, "urn:Person", &dict),
            t("urn:bob", rdf_type, "urn:Person", &dict),
            t("urn:carol", rdf_type, "urn:Person", &dict),
            t("urn:alice", "urn:ssn", "urn:v1", &dict),
            t("urn:bob", "urn:ssn", "urn:v1", &dict),
            t("urn:carol", "urn:ssn", "urn:v2", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        let same_as = format!("{owl}sameAs");
        let alice_bob = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:alice")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:bob"))
        });
        let carol_alice = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:carol")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:alice"))
        });
        assert!(alice_bob, "expected alice sameAs bob (single-key prp-key)");
        assert!(!carol_alice, "carol has a different key value");
    }

    #[test]
    fn has_key_literal_values_shared() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let rdf_nil = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
        let input = vec![
            t("urn:Person", &format!("{owl}hasKey"), "_:l1", &dict),
            t(
                "_:l1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#first",
                "urn:code",
                &dict,
            ),
            t(
                "_:l1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest",
                rdf_nil,
                &dict,
            ),
            t("urn:alice", rdf_type, "urn:Person", &dict),
            t("urn:bob", rdf_type, "urn:Person", &dict),
            t("urn:carol", rdf_type, "urn:Person", &dict),
            t("urn:dave", rdf_type, "urn:Person", &dict),
            tl("urn:alice", "urn:code", LiteralValue::Integer(42), &dict),
            tl("urn:bob", "urn:code", LiteralValue::Integer(42), &dict),
            tl("urn:carol", "urn:code", LiteralValue::Integer(43), &dict),
            tl("urn:dave", "urn:code", LiteralValue::Decimal(42.0), &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        let same_as = format!("{owl}sameAs");
        let alice_bob = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:alice")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:bob"))
        });
        let carol_alice = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:carol")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:alice"))
        });
        let dave_alice = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:dave")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:alice"))
        });
        assert!(
            alice_bob,
            "expected alice sameAs bob (shared literal key value)"
        );
        assert!(!carol_alice, "carol has a different literal value");
        assert!(
            !dave_alice,
            "no cross-datatype normalization: Integer vs Decimal do not match"
        );
    }

    #[test]
    fn different_from_reverse_direction_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let different_from = format!("{owl}differentFrom");
        let same_as = format!("{owl}sameAs");
        let input = vec![
            t("urn:y", &different_from, "urn:x", &dict),
            t("urn:x", &same_as, "urn:y", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        // eq-sym derives y sameAs x in the same iteration; ⊥ must fire even
        // with a single iteration budget (regression for F3).
        let outcome = reasoner
            .materialize(&dict, &task_max_iterations(1), &input)
            .expect("materialize");
        assert!(
            outcome.report.inconsistent,
            "expected eq-diff1 ⊥ after same-iteration eq-sym"
        );
    }

    #[test]
    fn subclass_chain_to_nothing_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("urn:A", &format!("{rdfs}subClassOf"), "urn:B", &dict),
            t(
                "urn:B",
                &format!("{rdfs}subClassOf"),
                &format!("{owl}Nothing"),
                &dict,
            ),
            t("urn:x", rdf_type, "urn:A", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        // rdfs5 derives A subClassOf Nothing in the same iteration; cls-nothing2
        // must fire even with a single iteration budget (regression for F3).
        let outcome = reasoner
            .materialize(&dict, &task_max_iterations(1), &input)
            .expect("materialize");
        assert!(
            outcome.report.inconsistent,
            "expected cls-nothing2 ⊥ after same-iteration rdfs5"
        );
    }

    #[test]
    fn has_key_cyclic_list_terminates() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("urn:Person", &format!("{owl}hasKey"), "_:l1", &dict),
            t(
                "_:l1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#first",
                "urn:ssn",
                &dict,
            ),
            tb(
                "_:l1",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest",
                "_:l2",
                &dict,
            ),
            t(
                "_:l2",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#first",
                "urn:email",
                &dict,
            ),
            tb(
                "_:l2",
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest",
                "_:l1",
                &dict,
            ),
            t("urn:alice", rdf_type, "urn:Person", &dict),
            t("urn:bob", rdf_type, "urn:Person", &dict),
            t("urn:alice", "urn:ssn", "urn:v1", &dict),
            t("urn:alice", "urn:email", "urn:e1", &dict),
            t("urn:bob", "urn:ssn", "urn:v1", &dict),
            t("urn:bob", "urn:email", "urn:e1", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        let same_as = format!("{owl}sameAs");
        assert!(outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:alice")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:bob"))
        }));
    }

    #[test]
    fn equality_replacement_propagates_values() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let same_as = format!("{owl}sameAs");
        let input = vec![
            t("urn:a", &same_as, "urn:b", &dict),
            t("urn:a", "urn:p", "urn:v", &dict),
            t("urn:x", "urn:q", "urn:a", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        let subject_replaced = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:b")
                && tr.predicate.as_str() == "urn:p"
                && tr.object == Term::Iri(Iri::new("urn:v"))
        });
        let object_replaced = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:x")
                && tr.predicate.as_str() == "urn:q"
                && tr.object == Term::Iri(Iri::new("urn:b"))
        });
        assert!(subject_replaced, "expected b p v (eq-rep-s)");
        assert!(object_replaced, "expected x q b (eq-rep-o)");
    }

    #[test]
    fn equality_replacement_applies_to_predicates() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let same_as = format!("{owl}sameAs");
        let input = vec![
            t("urn:p", &same_as, "urn:q", &dict),
            t("urn:x", "urn:p", "urn:v", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:x")
                && tr.predicate.as_str() == "urn:q"
                && tr.object == Term::Iri(Iri::new("urn:v"))
        }));
    }

    #[test]
    fn functional_property_infers_same_values() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let same_as = format!("{owl}sameAs");
        let input = vec![
            t(
                "urn:p",
                rdf_type,
                &format!("{owl}FunctionalProperty"),
                &dict,
            ),
            t("urn:x", "urn:p", "urn:y1", &dict),
            t("urn:x", "urn:p", "urn:y2", &dict),
            t(
                "urn:p2",
                rdf_type,
                &format!("{owl}InverseFunctionalProperty"),
                &dict,
            ),
            t("urn:x1", "urn:p2", "urn:v", &dict),
            t("urn:x2", "urn:p2", "urn:v", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        let fp = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:y1")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:y2"))
        });
        let ifp = outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:x1")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:x2"))
        });
        assert!(fp, "expected y1 sameAs y2 (prp-fp)");
        assert!(ifp, "expected x1 sameAs x2 (prp-ifp)");
    }

    #[test]
    fn complement_classes_mark_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("urn:Cat", &format!("{owl}complementOf"), "urn:Dog", &dict),
            t("urn:rex", rdf_type, "urn:Cat", &dict),
            t("urn:rex", rdf_type, "urn:Dog", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.report.inconsistent, "expected cls-com ⊥");
    }

    #[test]
    fn max_cardinality_one_infers_same_values() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let same_as = format!("{owl}sameAs");
        let input = vec![
            tl(
                "_:r",
                &format!("{owl}maxCardinality"),
                LiteralValue::Integer(1),
                &dict,
            ),
            t("_:r", &format!("{owl}onProperty"), "urn:p", &dict),
            tb("urn:x", rdf_type, "_:r", &dict),
            t("urn:x", "urn:p", "urn:y1", &dict),
            t("urn:x", "urn:p", "urn:y2", &dict),
        ];
        let reasoner = ForwardChainReasoner::new();
        let outcome = reasoner
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.triples.iter().any(|tr| {
            tr.subject == dict.encode_node("urn:y1")
                && tr.predicate.as_str() == same_as
                && tr.object == Term::Iri(Iri::new("urn:y2"))
        }));
    }

    // ---- Complete RDFS + OWL 2 RL rule set (P6-01 extension) ----

    fn has_triple(
        outcome: &[Triple],
        dict: &InMemoryDictionary,
        s: &str,
        p: &str,
        o: &str,
    ) -> bool {
        outcome.iter().any(|tr| {
            tr.subject == dict.encode_node(s)
                && tr.predicate.as_str() == p
                && tr.object == Term::Iri(Iri::new(o))
        })
    }

    fn has_literal_object(
        outcome: &[Triple],
        dict: &InMemoryDictionary,
        s: &str,
        p: &str,
        o: LiteralValue,
    ) -> bool {
        outcome.iter().any(|tr| {
            tr.subject == dict.encode_node(s)
                && tr.predicate.as_str() == p
                && tr.object == Term::Literal(o.clone())
        })
    }

    #[test]
    fn eq_ref_materializes_reflexive_same_as_for_connected_nodes() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let input = vec![t("urn:a", &format!("{owl}sameAs"), "urn:b", &dict)];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:a",
                &format!("{owl}sameAs"),
                "urn:a"
            ),
            "eq-ref: reflexive sameAs for sameAs-connected subject"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:b",
                &format!("{owl}sameAs"),
                "urn:b"
            ),
            "eq-ref: reflexive sameAs for sameAs-connected object"
        );
    }

    #[test]
    fn equivalent_property_derives_mutual_subproperty() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let owl = "http://www.w3.org/2002/07/owl#";
        let input = vec![
            t("urn:p", &format!("{owl}equivalentProperty"), "urn:q", &dict),
            t("urn:a", "urn:p", "urn:b", &dict),
        ];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:p",
                &format!("{rdfs}subPropertyOf"),
                "urn:q"
            ),
            "prp-eqp1/scm-eqp1: p ⊑ q"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:q",
                &format!("{rdfs}subPropertyOf"),
                "urn:p"
            ),
            "prp-eqp2/scm-eqp1: q ⊑ p"
        );
        assert!(
            has_triple(&outcome.triples, &dict, "urn:a", "urn:q", "urn:b"),
            "prp-spo1: equivalent property applies"
        );
    }

    #[test]
    fn mutual_subproperty_derives_equivalent_property() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let owl = "http://www.w3.org/2002/07/owl#";
        let input = vec![
            t("urn:p", &format!("{rdfs}subPropertyOf"), "urn:q", &dict),
            t("urn:q", &format!("{rdfs}subPropertyOf"), "urn:p", &dict),
        ];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:p",
                &format!("{owl}equivalentProperty"),
                "urn:q"
            ),
            "scm-eqp2: mutual ⊑ → equivalentProperty"
        );
    }

    #[test]
    fn property_schema_self_edges_and_domain_propagation() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("urn:p", rdf_type, &format!("{owl}ObjectProperty"), &dict),
            t("urn:q", rdf_type, &format!("{owl}DatatypeProperty"), &dict),
            t("urn:sub", &format!("{rdfs}subPropertyOf"), "urn:p", &dict),
            t("urn:p", &format!("{rdfs}domain"), "urn:C", &dict),
            t("urn:C", &format!("{rdfs}subClassOf"), "urn:D", &dict),
            t("urn:q", &format!("{rdfs}range"), "urn:E", &dict),
            t("urn:E", &format!("{rdfs}subClassOf"), "urn:F", &dict),
        ];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:p",
                &format!("{rdfs}subPropertyOf"),
                "urn:p"
            ),
            "scm-op: p ⊑ p"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:p",
                &format!("{owl}equivalentProperty"),
                "urn:p"
            ),
            "scm-op: p ≡ p"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:sub",
                &format!("{rdfs}domain"),
                "urn:C"
            ),
            "scm-dom2: domain propagates down subPropertyOf"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:p",
                &format!("{rdfs}domain"),
                "urn:D"
            ),
            "scm-dom1: domain propagates up subClassOf"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:q",
                &format!("{rdfs}range"),
                "urn:F"
            ),
            "scm-rng1: range propagates up subClassOf"
        );
    }

    #[test]
    fn class_schema_and_equivalent_class_rules() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("urn:C", rdf_type, &format!("{owl}Class"), &dict),
            t("urn:C", &format!("{owl}equivalentClass"), "urn:D", &dict),
            t("urn:x", rdf_type, "urn:C", &dict),
            t("urn:A", &format!("{rdfs}subClassOf"), "urn:B", &dict),
            t("urn:B", &format!("{rdfs}subClassOf"), "urn:A", &dict),
        ];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:C",
                &format!("{rdfs}subClassOf"),
                "urn:C"
            ),
            "scm-cls: c ⊑ c"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:C",
                &format!("{rdfs}subClassOf"),
                &format!("{owl}Thing")
            ),
            "scm-cls: c ⊑ owl:Thing"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                &format!("{owl}Nothing"),
                &format!("{rdfs}subClassOf"),
                "urn:C"
            ),
            "scm-cls: owl:Nothing ⊑ c"
        );
        assert!(
            has_triple(&outcome.triples, &dict, "urn:x", rdf_type, "urn:D"),
            "cax-eqc1/2: type propagates across equivalentClass"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:A",
                &format!("{owl}equivalentClass"),
                "urn:B"
            ),
            "scm-eqc2: mutual ⊑ → equivalentClass"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:C",
                &format!("{owl}equivalentClass"),
                "urn:D"
            ),
            "scm-eqc1: equivalentClass → mutual ⊑ (then scm-eqc2 back)"
        );
    }

    #[test]
    fn schema_rules_for_restriction_expressions() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let owl = "http://www.w3.org/2002/07/owl#";
        let mut input = vec![
            // scm-svf1: same property, subclassed fillers.
            t("_:s1", &format!("{owl}onProperty"), "urn:p", &dict),
            t("_:s1", &format!("{owl}someValuesFrom"), "urn:Y1", &dict),
            t("_:s2", &format!("{owl}onProperty"), "urn:p", &dict),
            t("_:s2", &format!("{owl}someValuesFrom"), "urn:Y2", &dict),
            t("urn:Y1", &format!("{rdfs}subClassOf"), "urn:Y2", &dict),
            // scm-avf2: same filler, subproperty.
            t("_:a1", &format!("{owl}onProperty"), "urn:q1", &dict),
            t("_:a1", &format!("{owl}allValuesFrom"), "urn:Z", &dict),
            t("_:a2", &format!("{owl}onProperty"), "urn:q2", &dict),
            t("_:a2", &format!("{owl}allValuesFrom"), "urn:Z", &dict),
            t("urn:q1", &format!("{rdfs}subPropertyOf"), "urn:q2", &dict),
        ];
        let rdf_first = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
        let rdf_rest = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
        let rdf_nil = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
        // scm-int: C ⊑ each intersection member; scm-uni: each union member ⊑ U.
        input.push(t(
            "urn:Inter",
            &format!("{owl}intersectionOf"),
            "_:il",
            &dict,
        ));
        input.push(t("_:il", rdf_first, "urn:C1", &dict));
        input.push(t("_:il", rdf_rest, rdf_nil, &dict));
        input.push(t("urn:Union", &format!("{owl}unionOf"), "_:ul", &dict));
        input.push(t("_:ul", rdf_first, "urn:C2", &dict));
        input.push(t("_:ul", rdf_rest, rdf_nil, &dict));
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:Inter",
                &format!("{rdfs}subClassOf"),
                "urn:C1"
            ),
            "scm-int: intersection ⊑ member"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:C2",
                &format!("{rdfs}subClassOf"),
                "urn:Union"
            ),
            "scm-uni: union member ⊑ union"
        );
        assert!(
            outcome.triples.iter().any(|tr| {
                tr.predicate == Iri::new(format!("{rdfs}subClassOf"))
                    && tr.subject == dict.encode_node("_:s1")
                    && tr.object == Term::BlankNode(dict.encode_node("_:s2"))
            }),
            "scm-svf1: same property + subclassed fillers → s1 ⊑ s2"
        );
        assert!(
            outcome.triples.iter().any(|tr| {
                tr.predicate == Iri::new(format!("{rdfs}subClassOf"))
                    && tr.subject == dict.encode_node("_:a2")
                    && tr.object == Term::BlankNode(dict.encode_node("_:a1"))
            }),
            "scm-avf2: same filler + subproperty → a2 ⊑ a1"
        );
    }

    #[test]
    fn one_of_types_its_members() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let mut input = vec![t("urn:Color", &format!("{owl}oneOf"), "_:cl", &dict)];
        input.extend(rdf_list(&dict, "_:cl", &["urn:red", "urn:green"]));
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            has_triple(&outcome.triples, &dict, "urn:red", rdf_type, "urn:Color"),
            "cls-oo: oneOf member typed"
        );
        assert!(
            has_triple(&outcome.triples, &dict, "urn:green", rdf_type, "urn:Color"),
            "cls-oo: second oneOf member typed"
        );
    }

    #[test]
    fn max_cardinality_zero_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("_:r", &format!("{owl}onProperty"), "urn:p", &dict),
            tl(
                "_:r",
                &format!("{owl}maxCardinality"),
                LiteralValue::Integer(0),
                &dict,
            ),
            tb("urn:x", rdf_type, "_:r", &dict),
            t("urn:x", "urn:p", "urn:y", &dict),
        ];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.report.inconsistent, "expected cls-maxc1 ⊥");
    }

    #[test]
    fn max_qualified_cardinality_zero_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        // cls-maxqc1: qualified value typed as the onClass filler.
        let input = vec![
            t("_:r", &format!("{owl}onProperty"), "urn:p", &dict),
            t("_:r", &format!("{owl}onClass"), "urn:C", &dict),
            tl(
                "_:r",
                &format!("{owl}maxQualifiedCardinality"),
                LiteralValue::Integer(0),
                &dict,
            ),
            tb("urn:x", rdf_type, "_:r", &dict),
            t("urn:x", "urn:p", "urn:y", &dict),
            t("urn:y", rdf_type, "urn:C", &dict),
        ];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.report.inconsistent, "expected cls-maxqc1 ⊥");
        // cls-maxqc2: unqualified (owl:Thing) value also violates.
        let input2 = vec![
            t("_:r", &format!("{owl}onProperty"), "urn:p", &dict),
            tl(
                "_:r",
                &format!("{owl}maxQualifiedCardinality"),
                LiteralValue::Integer(0),
                &dict,
            ),
            tb("urn:x", rdf_type, "_:r", &dict),
            t("urn:x", "urn:p", "urn:y", &dict),
        ];
        let outcome2 = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input2)
            .expect("materialize");
        assert!(outcome2.report.inconsistent, "expected cls-maxqc2 ⊥");
    }

    #[test]
    fn max_qualified_cardinality_one_infers_same_as() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let mut input = vec![
            t("_:r", &format!("{owl}onProperty"), "urn:p", &dict),
            t("_:r", &format!("{owl}onClass"), "urn:C", &dict),
            tl(
                "_:r",
                &format!("{owl}maxQualifiedCardinality"),
                LiteralValue::Integer(1),
                &dict,
            ),
            tb("urn:x", rdf_type, "_:r", &dict),
            t("urn:x", "urn:p", "urn:y1", &dict),
            t("urn:x", "urn:p", "urn:y2", &dict),
            t("urn:y1", rdf_type, "urn:C", &dict),
            t("urn:y2", rdf_type, "urn:C", &dict),
        ];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:y1",
                &format!("{owl}sameAs"),
                "urn:y2"
            ),
            "cls-maxqc3: qualified max 1 → sameAs"
        );
        // cls-maxqc4: owl:Thing filler without explicit typing.
        input = vec![
            t("_:r", &format!("{owl}onProperty"), "urn:p", &dict),
            tl(
                "_:r",
                &format!("{owl}maxQualifiedCardinality"),
                LiteralValue::Integer(1),
                &dict,
            ),
            tb("urn:x", rdf_type, "_:r", &dict),
            t("urn:x", "urn:p", "urn:z1", &dict),
            t("urn:x", "urn:p", "urn:z2", &dict),
        ];
        let outcome2 = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            has_triple(
                &outcome2.triples,
                &dict,
                "urn:z1",
                &format!("{owl}sameAs"),
                "urn:z2"
            ),
            "cls-maxqc4: unqualified max 1 → sameAs"
        );
    }

    #[test]
    fn asymmetric_property_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t(
                "urn:p",
                rdf_type,
                &format!("{owl}AsymmetricProperty"),
                &dict,
            ),
            t("urn:a", "urn:p", "urn:b", &dict),
            t("urn:b", "urn:p", "urn:a", &dict),
        ];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.report.inconsistent, "expected prp-asyp ⊥");
    }

    #[test]
    fn property_disjoint_with_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let input = vec![
            t(
                "urn:p1",
                &format!("{owl}propertyDisjointWith"),
                "urn:p2",
                &dict,
            ),
            t("urn:a", "urn:p1", "urn:b", &dict),
            t("urn:a", "urn:p2", "urn:b", &dict),
        ];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.report.inconsistent, "expected prp-pdw ⊥");
    }

    #[test]
    fn all_disjoint_properties_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let mut input = vec![
            t(
                "_:adp",
                rdf_type,
                &format!("{owl}AllDisjointProperties"),
                &dict,
            ),
            t("_:adp", &format!("{owl}members"), "_:l", &dict),
            t("urn:a", "urn:p1", "urn:b", &dict),
            t("urn:a", "urn:p2", "urn:b", &dict),
        ];
        input.extend(rdf_list(&dict, "_:l", &["urn:p1", "urn:p2"]));
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.report.inconsistent, "expected prp-adp ⊥");
    }

    #[test]
    fn negative_property_assertions_mark_inconsistent() {
        let dict = InMemoryDictionary::new();
        let owl = "http://www.w3.org/2002/07/owl#";
        let mut input = vec![
            t("urn:n1", &format!("{owl}sourceIndividual"), "urn:i", &dict),
            t("urn:n1", &format!("{owl}assertionProperty"), "urn:p", &dict),
            t("urn:n1", &format!("{owl}targetIndividual"), "urn:j", &dict),
            t("urn:i", "urn:p", "urn:j", &dict),
        ];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.report.inconsistent, "expected prp-npa1 ⊥");
        input = vec![
            t("urn:n2", &format!("{owl}sourceIndividual"), "urn:i", &dict),
            t("urn:n2", &format!("{owl}assertionProperty"), "urn:p", &dict),
            tl(
                "urn:n2",
                &format!("{owl}targetValue"),
                LiteralValue::Integer(7),
                &dict,
            ),
            tl("urn:i", "urn:p", LiteralValue::Integer(7), &dict),
        ];
        let outcome2 = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome2.report.inconsistent, "expected prp-npa2 ⊥");
    }

    #[test]
    fn datatype_not_type_marks_inconsistent() {
        let dict = InMemoryDictionary::new();
        let xsd = "http://www.w3.org/2001/XMLSchema#";
        let input = vec![tl(
            "urn:x",
            "urn:p",
            LiteralValue::Typed {
                value: "not-a-number".to_owned(),
                datatype: Iri::new(format!("{xsd}integer")),
            },
            &dict,
        )];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(outcome.report.inconsistent, "expected dt-not-type ⊥");
    }

    #[test]
    fn datatype_equality_duplicates_triples_for_equal_values() {
        let dict = InMemoryDictionary::new();
        let xsd = "http://www.w3.org/2001/XMLSchema#";
        let input = vec![
            tl(
                "urn:x",
                "urn:p",
                LiteralValue::Typed {
                    value: "01".to_owned(),
                    datatype: Iri::new(format!("{xsd}integer")),
                },
                &dict,
            ),
            tl("urn:y", "urn:q", LiteralValue::Integer(1), &dict),
        ];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            has_literal_object(
                &outcome.triples,
                &dict,
                "urn:x",
                "urn:p",
                LiteralValue::Integer(1),
            ),
            "dt-eq: '01'^^xsd:integer and 1 share a data value"
        );
    }

    #[test]
    fn rdfs_resource_typing_and_schema_reflexivity() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let rdf = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![
            t("urn:a", "urn:p", "urn:b", &dict),
            t("urn:p", rdf_type, &format!("{rdf}Property"), &dict),
            t("urn:C", rdf_type, &format!("{rdfs}Class"), &dict),
            t("urn:dt", rdf_type, &format!("{rdfs}Datatype"), &dict),
            t(
                "urn:cmp",
                rdf_type,
                &format!("{rdfs}ContainerMembershipProperty"),
                &dict,
            ),
        ];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:a",
                rdf_type,
                &format!("{rdfs}Resource")
            ),
            "rdfs4a: subject is a resource"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:b",
                rdf_type,
                &format!("{rdfs}Resource")
            ),
            "rdfs4b: object is a resource"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:p",
                &format!("{rdfs}subPropertyOf"),
                "urn:p"
            ),
            "rdfs6: property self-subPropertyOf"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:C",
                &format!("{rdfs}subClassOf"),
                &format!("{rdfs}Resource")
            ),
            "rdfs8: class ⊑ rdfs:Resource"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:C",
                &format!("{rdfs}subClassOf"),
                "urn:C"
            ),
            "rdfs10: class self-subClassOf"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:cmp",
                &format!("{rdfs}subPropertyOf"),
                &format!("{rdfs}member")
            ),
            "rdfs12: container membership property ⊑ rdfs:member"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "urn:dt",
                &format!("{rdfs}subClassOf"),
                &format!("{rdfs}Literal")
            ),
            "rdfs13: datatype ⊑ rdfs:Literal"
        );
    }

    #[test]
    fn axiomatic_rules_seed_background_but_do_not_inflate_inferred_count() {
        let dict = InMemoryDictionary::new();
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let owl = "http://www.w3.org/2002/07/owl#";
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let input = vec![t("urn:a", "urn:p", "urn:b", &dict)];
        let outcome = ForwardChainReasoner::new()
            .materialize(&dict, &task(InferenceMode::ForwardChaining), &input)
            .expect("materialize");
        // prp-ap: annotation properties typed.
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                &format!("{rdfs}label"),
                rdf_type,
                &format!("{owl}AnnotationProperty"),
            ),
            "prp-ap axiom"
        );
        // cls-thing / cls-nothing1.
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                &format!("{owl}Thing"),
                rdf_type,
                &format!("{owl}Class"),
            ),
            "cls-thing axiom"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                &format!("{owl}Nothing"),
                rdf_type,
                &format!("{owl}Class"),
            ),
            "cls-nothing1 axiom"
        );
        // dt-type1 + rdfs13 consequence.
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "http://www.w3.org/2001/XMLSchema#string",
                rdf_type,
                &format!("{rdfs}Datatype"),
            ),
            "dt-type1 axiom for xsd:string"
        );
        assert!(
            has_triple(
                &outcome.triples,
                &dict,
                "http://www.w3.org/2001/XMLSchema#integer",
                &format!("{rdfs}subClassOf"),
                &format!("{rdfs}Literal"),
            ),
            "rdfs13 for seeded datatype"
        );
        // Background axioms + rdfs4a/4b are not counted as inferences.
        assert!(
            outcome.report.inferred_triples == 0,
            "background-only materialization must report 0 inferred: {}",
            outcome.report.inferred_triples
        );
    }
}
