//! Rule-based SPARQL algebra optimizer (L3).

use crate::application::{QueryOptimizer, QueryStatistics};
use crate::domain::{Algebra, PatternCost, QueryPlan, TermPattern, TriplePattern};
use ontolith_core::error::OntolithError;
use std::collections::HashSet;
use std::sync::Arc;

/// Applies deterministic rewrite rules:
/// 1. Flatten nested Joins where possible (keep binary for executor simplicity)
/// 2. Push Filter through Project/Distinct when safe
/// 3. Reorder BGP triple patterns: bound-subject → bound-predicate → bound-object → unbound
/// 4. Merge consecutive BGPs inside Join(Bgp, Bgp)
/// 5. Eliminate Identity units
#[derive(Debug, Default, Clone, Copy)]
pub struct RuleBasedOptimizer;

/// Cost-based optimizer: same rewrite rules as [`RuleBasedOptimizer`] but
/// orders BGP triple patterns by estimated cardinality from live statistics,
/// preferring patterns that connect to already-selected bindings (join-order
/// with binding propagation).
pub struct CostBasedOptimizer<S: QueryStatistics> {
    stats: Arc<S>,
}

impl<S: QueryStatistics> CostBasedOptimizer<S> {
    pub fn new(stats: Arc<S>) -> Self {
        Self { stats }
    }
}

impl<S: QueryStatistics> QueryOptimizer for CostBasedOptimizer<S> {
    fn optimize(&self, mut plan: QueryPlan) -> Result<QueryPlan, OntolithError> {
        let before = crate::domain::summarize_algebra(&plan.algebra);
        plan.algebra = optimize_algebra_with_stats(plan.algebra, self.stats.as_ref());
        let after = crate::domain::summarize_algebra(&plan.algebra);
        let (estimated_rows, pattern_costs) =
            estimate_pattern_costs(&plan.algebra, self.stats.as_ref());
        plan.estimated_rows = estimated_rows;
        plan.pattern_costs = pattern_costs;
        plan.logical_steps
            .push(format!("optimize(cost):{before}->{after}"));
        plan.physical_steps =
            crate::infrastructure::sparql_parse::physical_steps_public(&plan.algebra);
        Ok(plan)
    }
}

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

/// [`optimize_algebra`] with statistics-driven BGP ordering.
pub fn optimize_algebra_with_stats(algebra: Algebra, stats: &dyn QueryStatistics) -> Algebra {
    let algebra = eliminate_identity(algebra);
    let algebra = reorder_and_merge_with_stats(algebra, stats);
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
            groups,
            aggregates,
            having,
            input,
        } => Algebra::Aggregate {
            groups: groups.clone(),
            aggregates: aggregates.clone(),
            having: having.clone(),
            input: Box::new(eliminate_identity(*input)),
        },
        Algebra::Path {
            subject,
            path,
            object,
        } => Algebra::Path {
            subject,
            path,
            object,
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
            groups,
            aggregates,
            having,
            input,
        } => Algebra::Aggregate {
            groups: groups.clone(),
            aggregates: aggregates.clone(),
            having: having.clone(),
            input: Box::new(reorder_and_merge(*input)),
        },
        Algebra::Path {
            subject,
            path,
            object,
        } => Algebra::Path {
            subject,
            path,
            object,
        },
        other => other,
    }
}

fn reorder_and_merge_with_stats(algebra: Algebra, stats: &dyn QueryStatistics) -> Algebra {
    match algebra {
        Algebra::Bgp(patterns) => Algebra::Bgp(reorder_bgp_cost(patterns, stats)),
        Algebra::Join { left, right } => {
            let l = reorder_and_merge_with_stats(*left, stats);
            let r = reorder_and_merge_with_stats(*right, stats);
            match (l, r) {
                (Algebra::Bgp(mut a), Algebra::Bgp(b)) => {
                    a.extend(b);
                    Algebra::Bgp(reorder_bgp_cost(a, stats))
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
            left: Box::new(reorder_and_merge_with_stats(*left, stats)),
            right: Box::new(reorder_and_merge_with_stats(*right, stats)),
            condition,
        },
        Algebra::Union { left, right } => Algebra::Union {
            left: Box::new(reorder_and_merge_with_stats(*left, stats)),
            right: Box::new(reorder_and_merge_with_stats(*right, stats)),
        },
        Algebra::Filter { expression, input } => Algebra::Filter {
            expression,
            input: Box::new(reorder_and_merge_with_stats(*input, stats)),
        },
        Algebra::Extend {
            variable,
            expression,
            input,
        } => Algebra::Extend {
            variable,
            expression,
            input: Box::new(reorder_and_merge_with_stats(*input, stats)),
        },
        Algebra::Distinct { input } => Algebra::Distinct {
            input: Box::new(reorder_and_merge_with_stats(*input, stats)),
        },
        Algebra::Project { variables, input } => Algebra::Project {
            variables,
            input: Box::new(reorder_and_merge_with_stats(*input, stats)),
        },
        Algebra::OrderBy { keys, input } => Algebra::OrderBy {
            keys,
            input: Box::new(reorder_and_merge_with_stats(*input, stats)),
        },
        Algebra::Slice {
            offset,
            limit,
            input,
        } => Algebra::Slice {
            offset,
            limit,
            input: Box::new(reorder_and_merge_with_stats(*input, stats)),
        },
        Algebra::Aggregate {
            groups,
            aggregates,
            having,
            input,
        } => Algebra::Aggregate {
            groups: groups.clone(),
            aggregates: aggregates.clone(),
            having: having.clone(),
            input: Box::new(reorder_and_merge_with_stats(*input, stats)),
        },
        Algebra::Path {
            subject,
            path,
            object,
        } => Algebra::Path {
            subject,
            path,
            object,
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

/// Greedy join-order: repeatedly pick the pattern with the lowest estimated
/// cardinality that shares a variable with the already-selected set (binding
/// propagation); otherwise the cheapest unbound pattern.
fn reorder_bgp_cost(
    patterns: Vec<TriplePattern>,
    stats: &dyn QueryStatistics,
) -> Vec<TriplePattern> {
    if patterns.len() < 2 {
        return patterns;
    }
    let mut remaining: Vec<TriplePattern> = patterns;
    let mut selected: Vec<TriplePattern> = Vec::with_capacity(remaining.len());
    let mut bound: HashSet<String> = HashSet::new();
    while !remaining.is_empty() {
        let mut best = 0usize;
        let mut best_sel = f64::INFINITY;
        let mut best_connects = false;
        for (i, p) in remaining.iter().enumerate() {
            let connects = pattern_vars(p).into_iter().any(|v| bound.contains(&v));
            let sel = stats.pattern_selectivity(p);
            // Prefer patterns that share bindings with the selected set;
            // within the same class pick the lowest estimated cardinality.
            if (connects && !best_connects) || (connects == best_connects && sel < best_sel) {
                best = i;
                best_sel = sel;
                best_connects = connects;
            }
        }
        let p = remaining.remove(best);
        for v in pattern_vars(&p) {
            bound.insert(v);
        }
        selected.push(p);
    }
    selected
}

fn pattern_vars(p: &TriplePattern) -> Vec<String> {
    let mut out = Vec::new();
    for t in [&p.subject, &p.predicate, &p.object] {
        if let TermPattern::Variable(v) | TermPattern::Blank(v) = t {
            out.push(v.clone());
        }
    }
    out
}

/// Flatten all BGP nodes of the algebra into per-pattern cost estimates and
/// derive a whole-query row estimate (the dominant BGP's product of pattern
/// selectivities × total triples, i.e. its expected output rows).
fn estimate_pattern_costs(
    algebra: &Algebra,
    stats: &dyn QueryStatistics,
) -> (Option<u64>, Vec<PatternCost>) {
    let total = stats.triple_count().max(1) as f64;
    let mut pattern_costs = Vec::new();
    let mut max_rows: Option<u64> = None;
    collect_bgp_estimates(algebra, stats, total, &mut pattern_costs, &mut max_rows);
    (max_rows, pattern_costs)
}

fn collect_bgp_estimates(
    algebra: &Algebra,
    stats: &dyn QueryStatistics,
    total: f64,
    out: &mut Vec<PatternCost>,
    max_rows: &mut Option<u64>,
) {
    match algebra {
        Algebra::Bgp(patterns) => {
            let mut product = 1.0;
            for p in patterns {
                let sel = stats.pattern_selectivity(p);
                product *= sel;
                out.push(PatternCost {
                    pattern: pattern_signature(p),
                    selectivity: sel,
                    estimated_rows: (sel * total).ceil() as u64,
                });
            }
            let rows = (product * total).ceil() as u64;
            *max_rows = Some(max_rows.unwrap_or(0).max(rows));
        }
        Algebra::Join { left, right }
        | Algebra::LeftJoin {
            left,
            right,
            condition: _,
        }
        | Algebra::Union { left, right } => {
            collect_bgp_estimates(left, stats, total, out, max_rows);
            collect_bgp_estimates(right, stats, total, out, max_rows);
        }
        Algebra::Filter { input, .. }
        | Algebra::Extend { input, .. }
        | Algebra::Distinct { input }
        | Algebra::Project { input, .. }
        | Algebra::OrderBy { input, .. }
        | Algebra::Slice { input, .. }
        | Algebra::Aggregate { input, .. } => {
            collect_bgp_estimates(input, stats, total, out, max_rows);
        }
        _ => {}
    }
}

fn pattern_signature(p: &TriplePattern) -> String {
    let sig = |t: &TermPattern| match t {
        TermPattern::Variable(v) | TermPattern::Blank(v) => format!("?{v}"),
        TermPattern::Iri(i) => format!("<{}>", i.as_str()),
        TermPattern::Node(n) => format!("node:{}", n.get()),
        TermPattern::Literal(l) => format!("{l:?}"),
    };
    format!("{} {} {}", sig(&p.subject), sig(&p.predicate), sig(&p.object))
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
            groups,
            aggregates,
            having,
            input,
        } => Algebra::Aggregate {
            groups: groups.clone(),
            aggregates: aggregates.clone(),
            having: having.clone(),
            input: Box::new(push_filters(*input)),
        },
        Algebra::Path {
            subject,
            path,
            object,
        } => Algebra::Path {
            subject,
            path,
            object,
        },
        other => other,
    }
}
