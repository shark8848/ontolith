//! Forward-chaining RDFS/OWL RL materializer (L6).
//!
//! Fixpoint iteration over a minimal rule set (rdfs5/6/7/8/9, prp-inv1)
//! with per-iteration dedup. Guards: `max_iterations` bounds the loop and
//! `InferenceMode::Off` short-circuits.

use crate::application::Reasoner;
use crate::domain::{MaterializeOutcome, ReasoningReport, ReasoningTask, Rule};
use ontolith_core::domain::{Iri, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_storage::application::DictionaryCodec;
use std::time::Instant;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
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
                },
            });
        }

        for _ in 0..task.max_iterations.max(1) {
            let mut frontier = Vec::new();
            apply_rules(dict, &closure, &mut frontier);
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
        ]
    }

    fn is_enabled(&self) -> bool {
        true
    }
}

fn apply_rules(dict: &dyn DictionaryCodec, closure: &[Triple], frontier: &mut Vec<Triple>) {
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
    use ontolith_storage::infrastructure::InMemoryDictionary;

    fn task(mode: InferenceMode) -> ReasoningTask {
        ReasoningTask {
            plan_id: None,
            mode,
            max_iterations: 16,
        }
    }

    fn t(s: &str, p: &str, o: &str, dict: &InMemoryDictionary) -> Triple {
        Triple::new(dict.encode_node(s), Iri::new(p), Term::Iri(Iri::new(o)))
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
}
