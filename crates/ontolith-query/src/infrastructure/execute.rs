//! Algebra executor producing solution bindings (L3).

use crate::application::QueryReadService;
use crate::domain::{
    AggregateFunction, Algebra, BoundValue, Expression, PathExpression, QueryKind, QueryPlan,
    QueryRequest, QueryResult, Solution, TermPattern, TriplePattern,
};
use ontolith_core::domain::{Iri, LiteralValue, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_transaction::domain::TxnId;
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use std::time::Instant;

pub struct AlgebraExecutor {
    read: Arc<dyn QueryReadService>,
}

impl AlgebraExecutor {
    pub fn new(read: Arc<dyn QueryReadService>) -> Self {
        Self { read }
    }

    pub fn execute(
        &self,
        plan: &QueryPlan,
        request: &QueryRequest,
    ) -> Result<QueryResult, OntolithError> {
        let started = Instant::now();
        if request.timeout_ms == Some(0) {
            return Ok(QueryResult {
                kind: plan.kind,
                variables: Vec::new(),
                solutions: Vec::new(),
                boolean: if plan.kind == QueryKind::Ask {
                    Some(false)
                } else {
                    None
                },
                construct_triples: Vec::new(),
                elapsed_ms: 0,
                timed_out: true,
                cancelled: false,
            });
        }
        if request.is_cancelled() {
            return Ok(empty_cancelled(plan.kind, started));
        }

        let ctx = ExecCtx {
            read: self.read.as_ref(),
            txn_id: request.txn_id,
            request,
            started,
        };

        let mut solutions = match eval_algebra(&plan.algebra, &ctx) {
            Ok(s) => s,
            Err(OntolithError::InvalidState("query timed out")) => {
                return Ok(QueryResult {
                    kind: plan.kind,
                    variables: Vec::new(),
                    solutions: Vec::new(),
                    boolean: if plan.kind == QueryKind::Ask {
                        Some(false)
                    } else {
                        None
                    },
                    construct_triples: Vec::new(),
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    timed_out: true,
                    cancelled: false,
                });
            }
            Err(OntolithError::InvalidState("query cancelled")) => {
                return Ok(empty_cancelled(plan.kind, started));
            }
            Err(e) => return Err(e),
        };

        let timed_out = request
            .timeout_ms
            .is_some_and(|t| started.elapsed().as_millis() as u64 > t);
        let cancelled = request.is_cancelled();

        match plan.kind {
            QueryKind::Ask => Ok(QueryResult {
                kind: plan.kind,
                variables: Vec::new(),
                solutions: Vec::new(),
                boolean: Some(!solutions.is_empty()),
                construct_triples: Vec::new(),
                elapsed_ms: started.elapsed().as_millis() as u64,
                timed_out,
                cancelled,
            }),
            QueryKind::Construct => {
                let triples = materialize_construct(&plan.construct_template, &solutions);
                Ok(QueryResult {
                    kind: plan.kind,
                    variables: Vec::new(),
                    solutions: Vec::new(),
                    boolean: None,
                    construct_triples: triples,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    timed_out,
                    cancelled,
                })
            }
            QueryKind::Select => {
                let mut variables = select_variables(&plan.algebra);
                if variables.is_empty() {
                    variables = collect_vars_from_solutions(&solutions);
                }
                if !variables.is_empty() {
                    for s in &mut solutions {
                        s.bindings.retain(|k, _| variables.contains(k));
                    }
                }
                Ok(QueryResult {
                    kind: plan.kind,
                    variables,
                    solutions,
                    boolean: None,
                    construct_triples: Vec::new(),
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    timed_out,
                    cancelled,
                })
            }
            other => Err(OntolithError::Unsupported(other.as_str())),
        }
    }
}

struct ExecCtx<'a> {
    read: &'a dyn QueryReadService,
    txn_id: Option<TxnId>,
    request: &'a QueryRequest,
    started: Instant,
}

impl ExecCtx<'_> {
    fn check(&self) -> Result<(), OntolithError> {
        if self.request.is_cancelled() {
            return Err(OntolithError::InvalidState("query cancelled"));
        }
        if let Some(limit) = self.request.timeout_ms
            && self.started.elapsed().as_millis() as u64 > limit
        {
            return Err(OntolithError::InvalidState("query timed out"));
        }
        Ok(())
    }
}

fn empty_cancelled(kind: QueryKind, started: Instant) -> QueryResult {
    QueryResult {
        kind,
        variables: Vec::new(),
        solutions: Vec::new(),
        boolean: if kind == QueryKind::Ask {
            Some(false)
        } else {
            None
        },
        construct_triples: Vec::new(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        timed_out: false,
        cancelled: true,
    }
}

fn eval_algebra(algebra: &Algebra, ctx: &ExecCtx<'_>) -> Result<Vec<Solution>, OntolithError> {
    ctx.check()?;
    match algebra {
        Algebra::Identity => Ok(vec![Solution::new()]),
        Algebra::Bgp(patterns) => eval_bgp(patterns, ctx),
        Algebra::Join { left, right } => {
            let l = eval_algebra(left, ctx)?;
            let r = eval_algebra(right, ctx)?;
            hash_join(l, r, ctx)
        }
        Algebra::LeftJoin {
            left,
            right,
            condition,
        } => {
            let l = eval_algebra(left, ctx)?;
            let r = eval_algebra(right, ctx)?;
            left_join(l, r, condition.as_ref(), ctx)
        }
        Algebra::Union { left, right } => {
            let mut l = eval_algebra(left, ctx)?;
            let r = eval_algebra(right, ctx)?;
            l.extend(r);
            Ok(l)
        }
        Algebra::Filter { expression, input } => {
            let rows = eval_algebra(input, ctx)?;
            Ok(rows
                .into_iter()
                .filter(|s| eval_expr_bool(expression, s).unwrap_or(false))
                .collect())
        }
        Algebra::Extend {
            variable,
            expression,
            input,
        } => {
            let mut rows = eval_algebra(input, ctx)?;
            for s in &mut rows {
                if let Some(v) = eval_expr_value(expression, s) {
                    s.insert(variable.clone(), v);
                }
            }
            Ok(rows)
        }
        Algebra::Values {
            variables,
            bindings,
        } => {
            let mut rows = Vec::new();
            for row in bindings {
                let mut s = Solution::new();
                for (i, var) in variables.iter().enumerate() {
                    if let Some(Some(term)) = row.get(i)
                        && let Some(bv) = term_pattern_to_bound(term)
                    {
                        s.insert(var.clone(), bv);
                    }
                }
                rows.push(s);
            }
            Ok(rows)
        }
        Algebra::Distinct { input } => {
            let rows = eval_algebra(input, ctx)?;
            let mut seen = HashSet::new();
            let mut out = Vec::new();
            for s in rows {
                let key = solution_key(&s);
                if seen.insert(key) {
                    out.push(s);
                }
            }
            Ok(out)
        }
        Algebra::Project { variables, input } => {
            let mut rows = eval_algebra(input, ctx)?;
            if !variables.is_empty() {
                for s in &mut rows {
                    s.bindings.retain(|k, _| variables.contains(k));
                }
            }
            Ok(rows)
        }
        Algebra::OrderBy { keys, input } => {
            let mut rows = eval_algebra(input, ctx)?;
            rows.sort_by(|a, b| {
                for key in keys {
                    let cmp = compare_bound(a.get(&key.variable), b.get(&key.variable));
                    let cmp = if key.ascending { cmp } else { cmp.reverse() };
                    if cmp != std::cmp::Ordering::Equal {
                        return cmp;
                    }
                }
                std::cmp::Ordering::Equal
            });
            Ok(rows)
        }
        Algebra::Slice {
            offset,
            limit,
            input,
        } => {
            let rows = eval_algebra(input, ctx)?;
            let skipped = rows.into_iter().skip(*offset);
            Ok(match limit {
                Some(n) => skipped.take(*n).collect(),
                None => skipped.collect(),
            })
        }
        Algebra::Aggregate {
            function,
            output,
            input,
        } => eval_aggregate(function, output, input, ctx),
        Algebra::Path {
            subject,
            path,
            object,
        } => eval_path_pattern(subject, path, object, ctx),
    }
}

fn eval_aggregate(
    function: &AggregateFunction,
    output: &str,
    input: &Algebra,
    ctx: &ExecCtx<'_>,
) -> Result<Vec<Solution>, OntolithError> {
    let rows = eval_algebra(input, ctx)?;
    let count = match function {
        AggregateFunction::Count { variable: None } => rows.len(),
        AggregateFunction::Count { variable: Some(v) } => {
            rows.iter().filter(|s| s.get(v).is_some()).count()
        }
    };

    let mut out = Solution::new();
    out.insert(output.to_owned(), BoundValue::Literal(LiteralValue::Integer(count as i64)));
    Ok(vec![out])
}

fn eval_path_pattern(
    subject: &TermPattern,
    path: &PathExpression,
    object: &TermPattern,
    ctx: &ExecCtx<'_>,
) -> Result<Vec<Solution>, OntolithError> {
    let starts = enumerate_path_starts(subject, ctx)?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for start in starts {
        ctx.check()?;
        let endpoints = eval_path_from_value(path, &start, ctx)?;
        for endpoint in endpoints {
            let mut row = Solution::new();
            if !bind_path_pattern(subject, &start, &mut row, ctx)? {
                continue;
            }
            if !bind_path_pattern(object, &endpoint, &mut row, ctx)? {
                continue;
            }
            let key = solution_key(&row);
            if seen.insert(key) {
                out.push(row);
            }
        }
    }

    Ok(out)
}

fn enumerate_path_starts(
    subject: &TermPattern,
    ctx: &ExecCtx<'_>,
) -> Result<Vec<BoundValue>, OntolithError> {
    if let Some(bound) = term_pattern_const_bound(subject) {
        return Ok(vec![normalize_path_value(bound, ctx)?]);
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let triples = ctx.read.all_triples(ctx.txn_id)?;
    for triple in triples {
        let subj = BoundValue::Node(triple.subject);
        let subj_key = path_value_key(&subj);
        if seen.insert(subj_key) {
            out.push(subj);
        }

        let obj = normalize_path_value(BoundValue::from_term(&triple.object), ctx)?;
        if !matches!(obj, BoundValue::Literal(_)) {
            let obj_key = path_value_key(&obj);
            if seen.insert(obj_key) {
                out.push(obj);
            }
        }
    }
    Ok(out)
}

fn eval_path_from_value(
    path: &PathExpression,
    start: &BoundValue,
    ctx: &ExecCtx<'_>,
) -> Result<Vec<BoundValue>, OntolithError> {
    ctx.check()?;
    match path {
        PathExpression::Predicate(predicate) => eval_predicate_from(start, predicate, ctx),
        PathExpression::InversePredicate(predicate) => {
            eval_inverse_predicate_from(start, predicate, ctx)
        }
        PathExpression::Sequence(left, right) => {
            let mids = eval_path_from_value(left, start, ctx)?;
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            for mid in mids {
                for end in eval_path_from_value(right, &mid, ctx)? {
                    let key = path_value_key(&end);
                    if seen.insert(key) {
                        out.push(end);
                    }
                }
            }
            Ok(out)
        }
        PathExpression::Alternative(left, right) => {
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            for value in eval_path_from_value(left, start, ctx)? {
                let key = path_value_key(&value);
                if seen.insert(key) {
                    out.push(value);
                }
            }
            for value in eval_path_from_value(right, start, ctx)? {
                let key = path_value_key(&value);
                if seen.insert(key) {
                    out.push(value);
                }
            }
            Ok(out)
        }
        PathExpression::OneOrMore(inner) => eval_one_or_more(inner, start, ctx),
        PathExpression::ZeroOrMore(inner) => {
            let mut out = vec![start.clone()];
            let mut seen = HashSet::new();
            seen.insert(path_value_key(start));
            for value in eval_one_or_more(inner, start, ctx)? {
                let key = path_value_key(&value);
                if seen.insert(key) {
                    out.push(value);
                }
            }
            Ok(out)
        }
    }
}

fn eval_one_or_more(
    inner: &PathExpression,
    start: &BoundValue,
    ctx: &ExecCtx<'_>,
) -> Result<Vec<BoundValue>, OntolithError> {
    let mut out = Vec::new();
    let mut out_seen = HashSet::new();
    let mut expanded = HashSet::new();
    let mut stack = vec![start.clone()];

    while let Some(current) = stack.pop() {
        ctx.check()?;
        let current_key = path_value_key(&current);
        if !expanded.insert(current_key) {
            continue;
        }

        for next in eval_path_from_value(inner, &current, ctx)? {
            let key = path_value_key(&next);
            if out_seen.insert(key.clone()) {
                out.push(next.clone());
            }
            stack.push(next);
        }
    }

    Ok(out)
}

fn eval_predicate_from(
    start: &BoundValue,
    predicate: &Iri,
    ctx: &ExecCtx<'_>,
) -> Result<Vec<BoundValue>, OntolithError> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for subject in subject_nodes_from_bound(start, ctx)? {
        ctx.check()?;
        for triple in ctx
            .read
            .matching(Some(subject), Some(predicate), None, ctx.txn_id)?
        {
            let value = normalize_path_value(BoundValue::from_term(&triple.object), ctx)?;
            let key = path_value_key(&value);
            if seen.insert(key) {
                out.push(value);
            }
        }
    }

    Ok(out)
}

fn eval_inverse_predicate_from(
    start: &BoundValue,
    predicate: &Iri,
    ctx: &ExecCtx<'_>,
) -> Result<Vec<BoundValue>, OntolithError> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for triple in ctx.read.matching(None, Some(predicate), None, ctx.txn_id)? {
        ctx.check()?;
        let candidate = normalize_path_value(BoundValue::from_term(&triple.object), ctx)?;
        if !bound_values_compatible(&candidate, start, ctx)? {
            continue;
        }
        let value = BoundValue::Node(triple.subject);
        let key = path_value_key(&value);
        if seen.insert(key) {
            out.push(value);
        }
    }

    Ok(out)
}

fn subject_nodes_from_bound(
    value: &BoundValue,
    ctx: &ExecCtx<'_>,
) -> Result<Vec<NodeId>, OntolithError> {
    match value {
        BoundValue::Node(n) | BoundValue::Blank(n) => Ok(vec![*n]),
        BoundValue::Iri(iri) => Ok(ctx.read.node_for_iri(iri)?.into_iter().collect()),
        BoundValue::Literal(_) => Ok(Vec::new()),
    }
}

fn bind_path_pattern(
    pattern: &TermPattern,
    value: &BoundValue,
    sol: &mut Solution,
    ctx: &ExecCtx<'_>,
) -> Result<bool, OntolithError> {
    match pattern {
        TermPattern::Variable(v) | TermPattern::Blank(v) => {
            if let Some(existing) = sol.get(v) {
                return bound_values_compatible(existing, value, ctx);
            }
            sol.insert(v.clone(), value.clone());
            Ok(true)
        }
        TermPattern::Node(expected) => match value {
            BoundValue::Node(actual) | BoundValue::Blank(actual) => Ok(actual == expected),
            BoundValue::Iri(actual) => {
                Ok(ctx.read.node_for_iri(actual)?.is_some_and(|n| n == *expected))
            }
            BoundValue::Literal(_) => Ok(false),
        },
        TermPattern::Iri(expected) => match value {
            BoundValue::Iri(actual) => Ok(actual == expected),
            BoundValue::Node(actual) | BoundValue::Blank(actual) => {
                Ok(ctx.read.node_for_iri(expected)?.is_some_and(|n| n == *actual))
            }
            BoundValue::Literal(_) => Ok(false),
        },
        TermPattern::Literal(expected) => match value {
            BoundValue::Literal(actual) => Ok(actual == expected),
            _ => Ok(false),
        },
    }
}

fn term_pattern_const_bound(pattern: &TermPattern) -> Option<BoundValue> {
    match pattern {
        TermPattern::Node(n) => Some(BoundValue::Node(*n)),
        TermPattern::Iri(i) => Some(BoundValue::Iri(i.clone())),
        TermPattern::Literal(l) => Some(BoundValue::Literal(l.clone())),
        TermPattern::Variable(_) | TermPattern::Blank(_) => None,
    }
}

fn path_value_key(value: &BoundValue) -> String {
    format!("{value:?}")
}

fn normalize_path_value(value: BoundValue, ctx: &ExecCtx<'_>) -> Result<BoundValue, OntolithError> {
    match value {
        BoundValue::Iri(iri) => {
            if let Some(node) = ctx.read.node_for_iri(&iri)? {
                Ok(BoundValue::Node(node))
            } else {
                Ok(BoundValue::Iri(iri))
            }
        }
        other => Ok(other),
    }
}

fn eval_bgp(patterns: &[TriplePattern], ctx: &ExecCtx<'_>) -> Result<Vec<Solution>, OntolithError> {
    if patterns.is_empty() {
        return Ok(vec![Solution::new()]);
    }
    let mut solutions = vec![Solution::new()];
    for pattern in patterns {
        ctx.check()?;
        let mut next = Vec::new();
        for sol in &solutions {
            let candidates = fetch_candidates(pattern, sol, ctx)?;
            for triple in candidates {
                if let Some(extended) = match_triple(pattern, &triple, sol, ctx)? {
                    next.push(extended);
                }
            }
        }
        solutions = next;
        if solutions.is_empty() {
            break;
        }
    }
    Ok(solutions)
}

fn fetch_candidates(
    pattern: &TriplePattern,
    sol: &Solution,
    ctx: &ExecCtx<'_>,
) -> Result<Vec<Triple>, OntolithError> {
    // Specialize pattern with current solution bindings, then use L2 multi-bound probe.
    let subj = bound_node(&pattern.subject, sol, ctx)?;
    let pred = bound_iri(&pattern.predicate, sol);
    let obj = bound_term(&pattern.object, sol);
    ctx.read
        .matching(subj, pred.as_ref(), obj.as_ref(), ctx.txn_id)
}

fn match_triple(
    pattern: &TriplePattern,
    triple: &Triple,
    sol: &Solution,
    ctx: &ExecCtx<'_>,
) -> Result<Option<Solution>, OntolithError> {
    let mut out = sol.clone();
    if bind_pattern(
        &pattern.subject,
        BoundValue::Node(triple.subject),
        &mut out,
        ctx,
    )?
    .is_none()
    {
        return Ok(None);
    }
    if bind_pattern(
        &pattern.predicate,
        BoundValue::Iri(triple.predicate.clone()),
        &mut out,
        ctx,
    )?
    .is_none()
    {
        return Ok(None);
    }
    if bind_pattern(
        &pattern.object,
        BoundValue::from_term(&triple.object),
        &mut out,
        ctx,
    )?
    .is_none()
    {
        return Ok(None);
    }
    Ok(Some(out))
}

fn bind_pattern(
    pattern: &TermPattern,
    value: BoundValue,
    sol: &mut Solution,
    ctx: &ExecCtx<'_>,
) -> Result<Option<()>, OntolithError> {
    match pattern {
        TermPattern::Variable(v) | TermPattern::Blank(v) => {
            if let Some(existing) = sol.get(v) {
                if existing == &value {
                    return Ok(Some(()));
                }

                if iri_node_compatible(existing, &value, ctx)? {
                    return Ok(Some(()));
                }

                return Ok(None);
            } else {
                sol.insert(v.clone(), value);
            }
            Ok(Some(()))
        }
        TermPattern::Node(n) => match value {
            BoundValue::Node(id) | BoundValue::Blank(id) if id == *n => Ok(Some(())),
            _ => Ok(None),
        },
        TermPattern::Iri(i) => match value {
            BoundValue::Iri(ref j) if j == i => Ok(Some(())),
            _ => Ok(None),
        },
        TermPattern::Literal(l) => match value {
            BoundValue::Literal(ref v) if v == l => Ok(Some(())),
            _ => Ok(None),
        },
    }
}

fn iri_node_compatible(
    left: &BoundValue,
    right: &BoundValue,
    ctx: &ExecCtx<'_>,
) -> Result<bool, OntolithError> {
    match (left, right) {
        (BoundValue::Iri(iri), BoundValue::Node(node) | BoundValue::Blank(node))
        | (BoundValue::Node(node) | BoundValue::Blank(node), BoundValue::Iri(iri)) => {
            Ok(ctx.read.node_for_iri(iri)?.is_some_and(|mapped| mapped == *node))
        }
        _ => Ok(false),
    }
}

fn bound_node(
    p: &TermPattern,
    sol: &Solution,
    ctx: &ExecCtx<'_>,
) -> Result<Option<NodeId>, OntolithError> {
    match p {
        TermPattern::Node(n) => Ok(Some(*n)),
        TermPattern::Variable(v) | TermPattern::Blank(v) => match sol.get(v) {
            Some(BoundValue::Node(n) | BoundValue::Blank(n)) => Ok(Some(*n)),
            Some(BoundValue::Iri(iri)) => ctx.read.node_for_iri(iri),
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}

fn bound_iri(p: &TermPattern, sol: &Solution) -> Option<Iri> {
    match p {
        TermPattern::Iri(i) => Some(i.clone()),
        TermPattern::Variable(v) => match sol.get(v) {
            Some(BoundValue::Iri(i)) => Some(i.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn bound_term(p: &TermPattern, sol: &Solution) -> Option<Term> {
    match p {
        TermPattern::Iri(i) => Some(Term::Iri(i.clone())),
        TermPattern::Literal(l) => Some(Term::Literal(l.clone())),
        TermPattern::Node(n) => Some(Term::BlankNode(*n)),
        TermPattern::Variable(v) | TermPattern::Blank(v) => sol.get(v).map(|b| match b {
            BoundValue::Iri(i) => Term::Iri(i.clone()),
            BoundValue::Literal(l) => Term::Literal(l.clone()),
            BoundValue::Node(n) | BoundValue::Blank(n) => Term::BlankNode(*n),
        }),
    }
}

fn hash_join(
    left: Vec<Solution>,
    right: Vec<Solution>,
    ctx: &ExecCtx<'_>,
) -> Result<Vec<Solution>, OntolithError> {
    let mut out = Vec::new();
    for l in &left {
        for r in &right {
            if let Some(m) = merge_solutions_compatible(l, r, ctx)? {
                out.push(m);
            }
        }
    }
    Ok(out)
}

fn left_join(
    left: Vec<Solution>,
    right: Vec<Solution>,
    condition: Option<&Expression>,
    ctx: &ExecCtx<'_>,
) -> Result<Vec<Solution>, OntolithError> {
    let mut out = Vec::new();
    for l in &left {
        let mut matched = false;
        for r in &right {
            if let Some(m) = merge_solutions_compatible(l, r, ctx)? {
                let ok = condition
                    .map(|c| eval_expr_bool(c, &m).unwrap_or(false))
                    .unwrap_or(true);
                if ok {
                    out.push(m);
                    matched = true;
                }
            }
        }
        if !matched {
            out.push(l.clone());
        }
    }
    Ok(out)
}

fn merge_solutions_compatible(
    left: &Solution,
    right: &Solution,
    ctx: &ExecCtx<'_>,
) -> Result<Option<Solution>, OntolithError> {
    let mut out = left.clone();
    for (var, value) in &right.bindings {
        if let Some(existing) = out.bindings.get(var) {
            if !bound_values_compatible(existing, value, ctx)? {
                return Ok(None);
            }
        } else {
            out.bindings.insert(var.clone(), value.clone());
        }
    }
    Ok(Some(out))
}

fn bound_values_compatible(
    left: &BoundValue,
    right: &BoundValue,
    ctx: &ExecCtx<'_>,
) -> Result<bool, OntolithError> {
    if left == right {
        return Ok(true);
    }

    match (left, right) {
        (BoundValue::Node(a) | BoundValue::Blank(a), BoundValue::Node(b) | BoundValue::Blank(b)) => {
            Ok(a == b)
        }
        _ => iri_node_compatible(left, right, ctx),
    }
}

fn eval_expr_bool(expr: &Expression, sol: &Solution) -> Option<bool> {
    match expr {
        Expression::Bound(v) => Some(sol.get(v).is_some()),
        Expression::Not(e) => Some(!eval_expr_bool(e, sol)?),
        Expression::And(a, b) => Some(eval_expr_bool(a, sol)? && eval_expr_bool(b, sol)?),
        Expression::Or(a, b) => Some(eval_expr_bool(a, sol)? || eval_expr_bool(b, sol)?),
        Expression::Equal(a, b) => Some(eval_expr_value(a, sol)? == eval_expr_value(b, sol)?),
        Expression::NotEqual(a, b) => Some(eval_expr_value(a, sol)? != eval_expr_value(b, sol)?),
        Expression::Less(a, b) => {
            Some(compare_values(&eval_expr_value(a, sol)?, &eval_expr_value(b, sol)?)? < 0)
        }
        Expression::LessEq(a, b) => {
            Some(compare_values(&eval_expr_value(a, sol)?, &eval_expr_value(b, sol)?)? <= 0)
        }
        Expression::Greater(a, b) => {
            Some(compare_values(&eval_expr_value(a, sol)?, &eval_expr_value(b, sol)?)? > 0)
        }
        Expression::GreaterEq(a, b) => {
            Some(compare_values(&eval_expr_value(a, sol)?, &eval_expr_value(b, sol)?)? >= 0)
        }
        Expression::IsIri(e) => Some(matches!(eval_expr_value(e, sol)?, BoundValue::Iri(_))),
        Expression::IsLiteral(e) => {
            Some(matches!(eval_expr_value(e, sol)?, BoundValue::Literal(_)))
        }
        Expression::IsBlank(e) => Some(matches!(
            eval_expr_value(e, sol)?,
            BoundValue::Blank(_) | BoundValue::Node(_)
        )),
        Expression::Variable(v) => sol.get(v).map(|_| true),
        Expression::Literal(LiteralValue::Boolean(b)) => Some(*b),
        _ => eval_expr_value(expr, sol).map(|_| true),
    }
}

fn eval_expr_value(expr: &Expression, sol: &Solution) -> Option<BoundValue> {
    match expr {
        Expression::Variable(v) => sol.get(v).cloned(),
        Expression::Iri(i) => Some(BoundValue::Iri(i.clone())),
        Expression::Literal(l) => Some(BoundValue::Literal(l.clone())),
        Expression::Bound(v) => Some(BoundValue::Literal(LiteralValue::Boolean(
            sol.get(v).is_some(),
        ))),
        Expression::Not(e) => Some(BoundValue::Literal(LiteralValue::Boolean(!eval_expr_bool(
            e, sol,
        )?))),
        Expression::And(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            eval_expr_bool(a, sol)? && eval_expr_bool(b, sol)?,
        ))),
        Expression::Or(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            eval_expr_bool(a, sol)? || eval_expr_bool(b, sol)?,
        ))),
        Expression::Equal(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            eval_expr_value(a, sol)? == eval_expr_value(b, sol)?,
        ))),
        Expression::NotEqual(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            eval_expr_value(a, sol)? != eval_expr_value(b, sol)?,
        ))),
        Expression::IsIri(e) => Some(BoundValue::Literal(LiteralValue::Boolean(matches!(
            eval_expr_value(e, sol)?,
            BoundValue::Iri(_)
        )))),
        Expression::IsLiteral(e) => Some(BoundValue::Literal(LiteralValue::Boolean(matches!(
            eval_expr_value(e, sol)?,
            BoundValue::Literal(_)
        )))),
        Expression::IsBlank(e) => Some(BoundValue::Literal(LiteralValue::Boolean(matches!(
            eval_expr_value(e, sol)?,
            BoundValue::Blank(_) | BoundValue::Node(_)
        )))),
        Expression::Less(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            compare_values(&eval_expr_value(a, sol)?, &eval_expr_value(b, sol)?)? < 0,
        ))),
        Expression::LessEq(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            compare_values(&eval_expr_value(a, sol)?, &eval_expr_value(b, sol)?)? <= 0,
        ))),
        Expression::Greater(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            compare_values(&eval_expr_value(a, sol)?, &eval_expr_value(b, sol)?)? > 0,
        ))),
        Expression::GreaterEq(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            compare_values(&eval_expr_value(a, sol)?, &eval_expr_value(b, sol)?)? >= 0,
        ))),
    }
}

fn compare_values(a: &BoundValue, b: &BoundValue) -> Option<i8> {
    match (a, b) {
        (
            BoundValue::Literal(LiteralValue::Integer(x)),
            BoundValue::Literal(LiteralValue::Integer(y)),
        ) => Some(match x.cmp(y) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }),
        (
            BoundValue::Literal(LiteralValue::Decimal(x)),
            BoundValue::Literal(LiteralValue::Decimal(y)),
        ) => x.partial_cmp(y).map(|o| match o {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }),
        (
            BoundValue::Literal(LiteralValue::String(x)),
            BoundValue::Literal(LiteralValue::String(y)),
        ) => Some(match x.cmp(y) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }),
        (BoundValue::Iri(x), BoundValue::Iri(y)) => Some(match x.as_str().cmp(y.as_str()) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }),
        _ => None,
    }
}

fn compare_bound(a: Option<&BoundValue>, b: Option<&BoundValue>) -> std::cmp::Ordering {
    match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(x), Some(y)) => match compare_values(x, y) {
            Some(-1) => std::cmp::Ordering::Less,
            Some(1) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        },
    }
}

fn term_pattern_to_bound(t: &TermPattern) -> Option<BoundValue> {
    match t {
        TermPattern::Iri(i) => Some(BoundValue::Iri(i.clone())),
        TermPattern::Literal(l) => Some(BoundValue::Literal(l.clone())),
        TermPattern::Node(n) => Some(BoundValue::Node(*n)),
        TermPattern::Blank(_) | TermPattern::Variable(_) => None,
    }
}

fn solution_key(s: &Solution) -> String {
    let mut parts = Vec::new();
    for (k, v) in &s.bindings {
        parts.push(format!("{k}={v:?}"));
    }
    parts.join("|")
}

fn select_variables(algebra: &Algebra) -> Vec<String> {
    match algebra {
        Algebra::Project { variables, .. } => variables.clone(),
        Algebra::Slice { input, .. }
        | Algebra::OrderBy { input, .. }
        | Algebra::Distinct { input }
        | Algebra::Filter { input, .. }
        | Algebra::Extend { input, .. }
        | Algebra::Aggregate { input, .. } => select_variables(input),
        Algebra::Path { subject, object, .. } => {
            let mut vars = BTreeSet::new();
            if let Some(v) = subject.as_variable() {
                vars.insert(v.to_owned());
            }
            if let Some(v) = object.as_variable() {
                vars.insert(v.to_owned());
            }
            vars.into_iter().collect()
        }
        _ => Vec::new(),
    }
}

fn collect_vars_from_solutions(solutions: &[Solution]) -> Vec<String> {
    let mut set = BTreeSet::new();
    for s in solutions {
        for k in s.bindings.keys() {
            set.insert(k.clone());
        }
    }
    set.into_iter().collect()
}

fn materialize_construct(template: &[TriplePattern], solutions: &[Solution]) -> Vec<Triple> {
    let mut out = Vec::new();
    for sol in solutions {
        for pattern in template {
            if let (Some(s), Some(p), Some(o)) = (
                instantiate_node(&pattern.subject, sol),
                instantiate_iri(&pattern.predicate, sol),
                instantiate_term(&pattern.object, sol),
            ) {
                out.push(Triple::new(s, p, o));
            }
        }
    }
    out
}

fn instantiate_node(p: &TermPattern, sol: &Solution) -> Option<NodeId> {
    match p {
        TermPattern::Node(n) => Some(*n),
        TermPattern::Variable(v) | TermPattern::Blank(v) => match sol.get(v)? {
            BoundValue::Node(n) | BoundValue::Blank(n) => Some(*n),
            _ => None,
        },
        _ => None,
    }
}

fn instantiate_iri(p: &TermPattern, sol: &Solution) -> Option<Iri> {
    match p {
        TermPattern::Iri(i) => Some(i.clone()),
        TermPattern::Variable(v) => match sol.get(v)? {
            BoundValue::Iri(i) => Some(i.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn instantiate_term(p: &TermPattern, sol: &Solution) -> Option<Term> {
    match p {
        TermPattern::Iri(i) => Some(Term::Iri(i.clone())),
        TermPattern::Literal(l) => Some(Term::Literal(l.clone())),
        TermPattern::Node(n) => Some(Term::BlankNode(*n)),
        TermPattern::Variable(v) | TermPattern::Blank(v) => match sol.get(v)? {
            BoundValue::Iri(i) => Some(Term::Iri(i.clone())),
            BoundValue::Literal(l) => Some(Term::Literal(l.clone())),
            BoundValue::Node(n) | BoundValue::Blank(n) => Some(Term::BlankNode(*n)),
        },
    }
}
