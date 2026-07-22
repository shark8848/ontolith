//! Rule-based SPARQL algebra optimizer (L3).

use crate::application::QueryOptimizer;
use crate::domain::{Algebra, QueryPlan};
use ontolith_core::error::OntolithError;

/// Applies deterministic rewrite rules:
/// 1. Flatten nested Joins where possible (keep binary for executor simplicity)
/// 2. Push Filter through Project/Distinct when safe
/// 3. Reorder BGP triple patterns: bound-subject → bound-predicate → bound-object → unbound
/// 4. Merge consecutive BGPs inside Join(Bgp, Bgp)
/// 5. Eliminate Identity units
#[derive(Debug, Default, Clone, Copy)]
pub struct RuleBasedOptimizer;

impl QueryOptimizer for RuleBasedOptimizer {
    fn optimize(&self, mut plan: QueryPlan) -> Result<QueryPlan, OntolithError> {
        let before = crate::domain::summarize_algebra(&plan.algebra);
        plan.algebra = optimize_algebra(plan.algebra);
        let after = crate::domain::summarize_algebra(&plan.algebra);
        plan.logical_steps
            .push(format!("optimize:{before}->{after}"));
        // refresh physical steps after rewrite
        plan.physical_steps =
            crate::infrastructure::sparql_parse::physical_steps_public(&plan.algebra);
        Ok(plan)
    }
}

pub fn optimize_algebra(algebra: Algebra) -> Algebra {
    let algebra = eliminate_identity(algebra);
    let algebra = reorder_and_merge(algebra);
    push_filters(algebra)
}

fn eliminate_identity(algebra: Algebra) -> Algebra {
    match algebra {
        Algebra::Join { left, right } => {
            let l = eliminate_identity(*left);
            let r = eliminate_identity(*right);
            match (l, r) {
                (Algebra::Identity, x) | (x, Algebra::Identity) => x,
                (l, r) => Algebra::Join {
                    left: Box::new(l),
                    right: Box::new(r),
                },
            }
        }
        Algebra::LeftJoin {
            left,
            right,
            condition,
        } => Algebra::LeftJoin {
            left: Box::new(eliminate_identity(*left)),
            right: Box::new(eliminate_identity(*right)),
            condition,
        },
        Algebra::Union { left, right } => Algebra::Union {
            left: Box::new(eliminate_identity(*left)),
            right: Box::new(eliminate_identity(*right)),
        },
        Algebra::Filter { expression, input } => Algebra::Filter {
            expression,
            input: Box::new(eliminate_identity(*input)),
        },
        Algebra::Extend {
            variable,
            expression,
            input,
        } => Algebra::Extend {
            variable,
            expression,
            input: Box::new(eliminate_identity(*input)),
        },
        Algebra::Distinct { input } => Algebra::Distinct {
            input: Box::new(eliminate_identity(*input)),
        },
        Algebra::Project { variables, input } => Algebra::Project {
            variables,
            input: Box::new(eliminate_identity(*input)),
        },
        Algebra::OrderBy { keys, input } => Algebra::OrderBy {
            keys,
            input: Box::new(eliminate_identity(*input)),
        },
        Algebra::Slice {
            offset,
            limit,
            input,
        } => Algebra::Slice {
            offset,
            limit,
            input: Box::new(eliminate_identity(*input)),
        },
        Algebra::Aggregate {
            function,
            output,
            input,
        } => Algebra::Aggregate {
            function,
            output,
            input: Box::new(eliminate_identity(*input)),
        },
        other => other,
    }
}

fn reorder_and_merge(algebra: Algebra) -> Algebra {
    match algebra {
        Algebra::Bgp(patterns) => Algebra::Bgp(reorder_bgp(patterns)),
        Algebra::Join { left, right } => {
            let l = reorder_and_merge(*left);
            let r = reorder_and_merge(*right);
            match (l, r) {
                (Algebra::Bgp(mut a), Algebra::Bgp(b)) => {
                    a.extend(b);
                    Algebra::Bgp(reorder_bgp(a))
                }
                (l, r) => Algebra::Join {
                    left: Box::new(l),
                    right: Box::new(r),
                },
            }
        }
        Algebra::LeftJoin {
            left,
            right,
            condition,
        } => Algebra::LeftJoin {
            left: Box::new(reorder_and_merge(*left)),
            right: Box::new(reorder_and_merge(*right)),
            condition,
        },
        Algebra::Union { left, right } => Algebra::Union {
            left: Box::new(reorder_and_merge(*left)),
            right: Box::new(reorder_and_merge(*right)),
        },
        Algebra::Filter { expression, input } => Algebra::Filter {
            expression,
            input: Box::new(reorder_and_merge(*input)),
        },
        Algebra::Extend {
            variable,
            expression,
            input,
        } => Algebra::Extend {
            variable,
            expression,
            input: Box::new(reorder_and_merge(*input)),
        },
        Algebra::Distinct { input } => Algebra::Distinct {
            input: Box::new(reorder_and_merge(*input)),
        },
        Algebra::Project { variables, input } => Algebra::Project {
            variables,
            input: Box::new(reorder_and_merge(*input)),
        },
        Algebra::OrderBy { keys, input } => Algebra::OrderBy {
            keys,
            input: Box::new(reorder_and_merge(*input)),
        },
        Algebra::Slice {
            offset,
            limit,
            input,
        } => Algebra::Slice {
            offset,
            limit,
            input: Box::new(reorder_and_merge(*input)),
        },
        Algebra::Aggregate {
            function,
            output,
            input,
        } => Algebra::Aggregate {
            function,
            output,
            input: Box::new(reorder_and_merge(*input)),
        },
        other => other,
    }
}

fn reorder_bgp(
    mut patterns: Vec<crate::domain::TriplePattern>,
) -> Vec<crate::domain::TriplePattern> {
    patterns.sort_by_key(pattern_rank);
    patterns
}

fn pattern_rank(p: &crate::domain::TriplePattern) -> u8 {
    let s = !p.subject.is_variable();
    let pred = !p.predicate.is_variable();
    let o = !p.object.is_variable();
    match (s, pred, o) {
        (true, _, _) => 0,         // subject bound → SPO
        (false, true, _) => 1,     // predicate bound → POS
        (false, false, true) => 2, // object bound → OSP
        _ => 3,
    }
}

fn push_filters(algebra: Algebra) -> Algebra {
    match algebra {
        Algebra::Filter { expression, input } => {
            let input = push_filters(*input);
            // Push through Distinct
            if let Algebra::Distinct { input: inner } = input {
                return Algebra::Distinct {
                    input: Box::new(push_filters(Algebra::Filter {
                        expression,
                        input: inner,
                    })),
                };
            }
            Algebra::Filter {
                expression,
                input: Box::new(input),
            }
        }
        Algebra::Join { left, right } => Algebra::Join {
            left: Box::new(push_filters(*left)),
            right: Box::new(push_filters(*right)),
        },
        Algebra::LeftJoin {
            left,
            right,
            condition,
        } => Algebra::LeftJoin {
            left: Box::new(push_filters(*left)),
            right: Box::new(push_filters(*right)),
            condition,
        },
        Algebra::Union { left, right } => Algebra::Union {
            left: Box::new(push_filters(*left)),
            right: Box::new(push_filters(*right)),
        },
        Algebra::Extend {
            variable,
            expression,
            input,
        } => Algebra::Extend {
            variable,
            expression,
            input: Box::new(push_filters(*input)),
        },
        Algebra::Distinct { input } => Algebra::Distinct {
            input: Box::new(push_filters(*input)),
        },
        Algebra::Project { variables, input } => Algebra::Project {
            variables,
            input: Box::new(push_filters(*input)),
        },
        Algebra::OrderBy { keys, input } => Algebra::OrderBy {
            keys,
            input: Box::new(push_filters(*input)),
        },
        Algebra::Slice {
            offset,
            limit,
            input,
        } => Algebra::Slice {
            offset,
            limit,
            input: Box::new(push_filters(*input)),
        },
        Algebra::Aggregate {
            function,
            output,
            input,
        } => Algebra::Aggregate {
            function,
            output,
            input: Box::new(push_filters(*input)),
        },
        other => other,
    }
}
