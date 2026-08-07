//! Forward-chaining RDFS/OWL RL materializer (L6).
//!
//! Fixpoint iteration over the supported rule set (rdfs5/6/7/8/9, prp-inv1/2,
//! prp-symp, prp-trp, cax-sco) with per-iteration dedup. Guards:
//! `max_iterations` and `max_elapsed_ms` bound the loop and
//! `InferenceMode::Off` short-circuits.

mod shacl;

use crate::application::Reasoner;
use crate::domain::{MaterializeOutcome, ReasoningReport, ReasoningTask, Rule};
use ontolith_core::domain::{Iri, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_storage::application::DictionaryCodec;
use std::time::Instant;

pub use shacl::ShaclEngine;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDFS_NS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const OWL_NS: &str = "http://www.w3.org/2002/07/owl#";

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

        let inferred = closure.len().saturating_sub(input.len());
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
        vec![
            Rule::SubClassOfTransitive,
            Rule::SubPropertyOfTransitive,
            Rule::SubPropertyOf,
            Rule::Domain,
            Rule::Range,
            Rule::InverseOf,
            Rule::SubClassOf,
            Rule::SymmetricProperty,
            Rule::TransitiveProperty,
            Rule::InverseOfReverse,
            Rule::HasKey,
            Rule::SomeValuesFrom,
            Rule::SomeValuesFromTyping,
            Rule::AllValuesFrom,
            Rule::IntersectionOf,
            Rule::IntersectionOfTyping,
            Rule::UnionOf,
            Rule::SameAsSymmetric,
            Rule::SameAsTransitive,
            Rule::DisjointClasses,
            Rule::NothingTyping,
            Rule::NothingSubClass,
            Rule::DifferentFromSelf,
            Rule::SameAsDifferentFrom,
        ]
    }

    fn is_enabled(&self) -> bool {
        true
    }
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

    // Members of an RDF list starting at `start`.
    let list_members = |start: &Term| -> Vec<Iri> {
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
                .and_then(|t| iri_of(&t.object));
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
    }
    for t in &all {
        if t.predicate != different_from {
            continue;
        }
        let Some(obj_node) = subject_node(&t.object) else {
            continue;
        };
        // eq-diff1: x owl:differentFrom x → ⊥.
        if t.subject == obj_node {
            *inconsistent = true;
        }
        // eq-diff2: x owl:sameAs y ∧ x owl:differentFrom y → ⊥.
        if same_as_all
            .iter()
            .any(|(a, b)| *a == t.subject && *b == obj_node)
        {
            *inconsistent = true;
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
        for expected in [
            "rdfs5",
            "rdfs6",
            "rdfs7",
            "rdfs8",
            "rdfs9",
            "prp-inv1",
            "prp-inv2",
            "prp-symp",
            "prp-trp",
            "cax-sco",
            "cls-svf1",
            "cls-svf2",
            "cls-avf",
            "cls-int1",
            "cls-int2",
            "cls-uni",
            "eq-sym",
            "eq-trans",
            "prp-key",
            "cax-dw",
            "cls-nothing1",
            "cls-nothing2",
            "eq-diff1",
            "eq-diff2",
        ] {
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
        assert!(outcome.report.inconsistent, "expected eq-diff1/2 ⊥");
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
            "expected eq-diff2 ⊥ after same-iteration eq-sym"
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
}
