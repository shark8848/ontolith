//! Algebra executor producing solution bindings (L3).

use crate::application::{QueryReadService, UpdateWriteService};
use crate::domain::{
    AggregateFunction, AggregateSpec, Algebra, BoundValue, Expression, GraphTarget, PathExpression,
    PreemptionReason, PreemptionToken, QueryKind, QueryPlan, QueryRequest, QueryResult, Solution,
    TermPattern, TriplePattern, UpdateOp,
};
use ontolith_core::domain::{Iri, LanguageTag, LiteralValue, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Quad, Term, Triple};
use ontolith_storage::domain::WriteOperation;
use ontolith_transaction::domain::TxnId;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
                affected: 0,
                elapsed_ms: 0,
                timed_out: true,
                cancelled: false,
            });
        }
        if request.is_cancelled() {
            return Ok(empty_cancelled(plan.kind, started));
        }

        let token = request.preemption_token();
        let ctx = ExecCtx {
            read: self.read.as_ref(),
            txn_id: request.txn_id,
            token: &token,
            base: plan.base.as_deref(),
        };

        // Projection expressions must see every variable bound by the WHERE
        // clause, so drop the SELECT Project layer before evaluation; the
        // Select arm below re-applies the projection.
        let eval_target = if plan.kind == QueryKind::Select && !plan.projection_exprs.is_empty() {
            strip_select_projection(&plan.algebra)
        } else {
            plan.algebra.clone()
        };
        let mut solutions = match eval_algebra(&eval_target, &ctx) {
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
                    affected: 0,
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
                affected: 0,
                elapsed_ms: started.elapsed().as_millis() as u64,
                timed_out,
                cancelled,
            }),
            QueryKind::Construct => {
                let triples = materialize_construct(&plan.construct_template, &solutions, &ctx);
                Ok(QueryResult {
                    kind: plan.kind,
                    variables: Vec::new(),
                    solutions: Vec::new(),
                    boolean: None,
                    construct_triples: triples,
                    affected: 0,
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
                // Evaluate SELECT projection expressions `(expr AS ?alias)` per
                // solution before trimming non-projected bindings.
                if !plan.projection_exprs.is_empty() {
                    for s in &mut solutions {
                        for pe in &plan.projection_exprs {
                            if let Some(v) = eval_expr_value(&pe.expression, s, &ctx) {
                                s.insert(pe.alias.clone(), v);
                            }
                        }
                    }
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
                    affected: 0,
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
    token: &'a PreemptionToken,
    base: Option<&'a str>,
}

impl ExecCtx<'_> {
    fn check(&self) -> Result<(), OntolithError> {
        if let Some(reason) = self.token.reason() {
            return Err(match reason {
                PreemptionReason::Cancelled => OntolithError::InvalidState("query cancelled"),
                PreemptionReason::Timeout => OntolithError::InvalidState("query timed out"),
            });
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
        affected: 0,
        elapsed_ms: started.elapsed().as_millis() as u64,
        timed_out: false,
        cancelled: true,
    }
}

/// SPARQL Update execution: resolves concrete triples, evaluates DELETE/INSERT
/// templates against the WHERE solutions, and applies writes in one txn.
pub fn execute_update(
    plan: &QueryPlan,
    request: &QueryRequest,
    read: &dyn QueryReadService,
    write: &dyn UpdateWriteService,
) -> Result<QueryResult, OntolithError> {
    let started = Instant::now();
    let txn_id = next_update_txn();
    let token = request.preemption_token();
    let ctx = ExecCtx {
        read,
        txn_id: Some(txn_id),
        token: &token,
        base: None,
    };
    let mut affected: u64 = 0;
    let mut staged = false;

    let result =
        (|| -> Result<(), OntolithError> {
            for op in &plan.update_ops {
                match op {
                    UpdateOp::InsertData(patterns) => {
                        let triples = concrete_update_triples(patterns, read)?;
                        let ops: Vec<_> = triples
                            .iter()
                            .map(|t| WriteOperation::PutTriple(t.clone()))
                            .collect();
                        if !ops.is_empty() {
                            write.apply_write_batch(txn_id, ops)?;
                            staged = true;
                        }
                        affected += triples.len() as u64;
                    }
                    UpdateOp::DeleteData(patterns) => {
                        let triples = concrete_update_triples(patterns, read)?;
                        let ops: Vec<_> = triples
                            .iter()
                            .map(|t| WriteOperation::DeleteTriple(t.clone()))
                            .collect();
                        if !ops.is_empty() {
                            write.apply_write_batch(txn_id, ops)?;
                            staged = true;
                        }
                        affected += triples.len() as u64;
                    }
                    UpdateOp::DeleteInsert {
                        graph,
                        delete,
                        insert,
                        where_pattern,
                    } => {
                        let scoped = graph.as_ref().map(|g| GraphScopedRead::new(read, write, g));
                        let op_read: &dyn QueryReadService = scoped
                            .as_ref()
                            .map(|s| s as &dyn QueryReadService)
                            .unwrap_or(read);
                        let op_ctx = ExecCtx {
                            read: op_read,
                            txn_id: Some(txn_id),
                            token: &token,
                            base: None,
                        };
                        let solutions = eval_algebra(where_pattern, &op_ctx)?;
                        let mut ops = Vec::new();
                        let mut seen = HashSet::new();
                        for sol in &solutions {
                            for t in materialize_update_triples(delete, sol, op_read)? {
                                let key = match graph {
                                    Some(_) => format!("g|{}", triple_key(&t)),
                                    None => triple_key(&t),
                                };
                                if seen.insert(key) {
                                    ops.push(match graph {
                                        Some(g) => WriteOperation::DeleteQuad(
                                            Quad::in_named_graph(t, g.clone()),
                                        ),
                                        None => WriteOperation::DeleteTriple(t),
                                    });
                                }
                            }
                            for t in materialize_update_triples(insert, sol, op_read)? {
                                ops.push(match graph {
                                    Some(g) => {
                                        WriteOperation::PutQuad(Quad::in_named_graph(t, g.clone()))
                                    }
                                    None => WriteOperation::PutTriple(t),
                                });
                            }
                        }
                        if !ops.is_empty() {
                            affected += ops.len() as u64;
                            write.apply_write_batch(txn_id, ops)?;
                            staged = true;
                        }
                    }
                    UpdateOp::DeleteWhere { graph, patterns } => {
                        let scoped = graph.as_ref().map(|g| GraphScopedRead::new(read, write, g));
                        let op_read: &dyn QueryReadService = scoped
                            .as_ref()
                            .map(|s| s as &dyn QueryReadService)
                            .unwrap_or(read);
                        let op_ctx = ExecCtx {
                            read: op_read,
                            txn_id: Some(txn_id),
                            token: &token,
                            base: None,
                        };
                        let solutions = eval_algebra(&Algebra::Bgp(patterns.clone()), &op_ctx)?;
                        let mut ops = Vec::new();
                        let mut seen = HashSet::new();
                        for sol in &solutions {
                            for t in materialize_update_triples(patterns, sol, op_read)? {
                                let key = match graph {
                                    Some(_) => format!("g|{}", triple_key(&t)),
                                    None => triple_key(&t),
                                };
                                if seen.insert(key) {
                                    ops.push(match graph {
                                        Some(g) => WriteOperation::DeleteQuad(
                                            Quad::in_named_graph(t, g.clone()),
                                        ),
                                        None => WriteOperation::DeleteTriple(t),
                                    });
                                }
                            }
                        }
                        if !ops.is_empty() {
                            affected += ops.len() as u64;
                            write.apply_write_batch(txn_id, ops)?;
                            staged = true;
                        }
                    }
                    UpdateOp::Clear { target, .. } | UpdateOp::Drop { target, .. } => {
                        let mut ops = Vec::new();
                        match target {
                            GraphTarget::Default => {
                                for t in write.default_graph_triples(Some(txn_id)) {
                                    ops.push(WriteOperation::DeleteTriple(t));
                                }
                            }
                            GraphTarget::Graph(g) => {
                                for q in write.quads_in_graph(g, Some(txn_id)) {
                                    ops.push(WriteOperation::DeleteQuad(q));
                                }
                            }
                            GraphTarget::Named => {
                                for q in write.named_graph_quads() {
                                    ops.push(WriteOperation::DeleteQuad(q));
                                }
                            }
                            GraphTarget::All => {
                                for t in write.default_graph_triples(Some(txn_id)) {
                                    ops.push(WriteOperation::DeleteTriple(t));
                                }
                                for q in write.named_graph_quads() {
                                    ops.push(WriteOperation::DeleteQuad(q));
                                }
                            }
                        }
                        if !ops.is_empty() {
                            affected += ops.len() as u64;
                            write.apply_write_batch(txn_id, ops)?;
                            staged = true;
                        }
                    }
                    UpdateOp::Load { source, into, .. } => {
                        let quads = write.quads_in_graph(source, Some(txn_id));
                        let mut ops = Vec::new();
                        match into {
                            Some(g) => {
                                for q in quads {
                                    ops.push(WriteOperation::PutQuad(Quad::in_named_graph(
                                        q.triple,
                                        g.clone(),
                                    )));
                                }
                            }
                            None => {
                                for q in quads {
                                    ops.push(WriteOperation::PutTriple(q.triple));
                                }
                            }
                        }
                        if !ops.is_empty() {
                            affected += ops.len() as u64;
                            write.apply_write_batch(txn_id, ops)?;
                            staged = true;
                        }
                    }
                }
                ctx.check()?;
            }
            Ok(())
        })();

    let (timed_out, cancelled) = match result {
        Ok(()) => {
            if staged {
                write.commit(txn_id)?;
            }
            (false, false)
        }
        Err(e) => {
            if staged {
                let _ = write.abort(txn_id);
            }
            match &e {
                OntolithError::InvalidState(s) if *s == "query timed out" => (true, false),
                OntolithError::InvalidState(s) if *s == "query cancelled" => (false, true),
                _ => return Err(e),
            }
        }
    };

    Ok(QueryResult {
        kind: QueryKind::Update,
        variables: Vec::new(),
        solutions: Vec::new(),
        boolean: None,
        construct_triples: Vec::new(),
        affected,
        elapsed_ms: started.elapsed().as_millis() as u64,
        timed_out,
        cancelled,
    })
}

static UPDATE_TXN_COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_update_txn() -> TxnId {
    // Offset range keeps L3-generated txn ids disjoint from L5 txn manager ids.
    let n = UPDATE_TXN_COUNTER.fetch_add(1, Ordering::Relaxed);
    TxnId::new(0x8000_0000_0000_0000 + n as u128)
}

fn concrete_update_triples(
    patterns: &[TriplePattern],
    read: &dyn QueryReadService,
) -> Result<Vec<Triple>, OntolithError> {
    let mut out = Vec::new();
    for p in patterns {
        let triples = materialize_update_triples(std::slice::from_ref(p), &Solution::new(), read)?;
        if triples.is_empty() {
            return Err(OntolithError::query(
                "DATA block triples must be concrete (no variables)",
            ));
        }
        out.extend(triples);
    }
    Ok(out)
}

fn materialize_update_triples(
    patterns: &[TriplePattern],
    sol: &Solution,
    read: &dyn QueryReadService,
) -> Result<Vec<Triple>, OntolithError> {
    let mut out = Vec::new();
    for p in patterns {
        let subject = match &p.subject {
            TermPattern::Node(n) => *n,
            TermPattern::Iri(i) => read.encode_node(i.as_str()).ok_or_else(|| {
                OntolithError::query(format!(
                    "cannot resolve subject <{}> to a node id (missing dictionary bridge)",
                    i.as_str()
                ))
            })?,
            TermPattern::Variable(v) | TermPattern::Blank(v) => match sol.get(v) {
                Some(BoundValue::Node(n) | BoundValue::Blank(n)) => *n,
                _ => continue,
            },
            TermPattern::Literal(_) => {
                return Err(OntolithError::query("literal cannot be a triple subject"));
            }
        };
        let predicate = match &p.predicate {
            TermPattern::Iri(i) => i.clone(),
            TermPattern::Variable(v) | TermPattern::Blank(v) => match sol.get(v) {
                Some(BoundValue::Iri(i)) => i.clone(),
                _ => continue,
            },
            _ => return Err(OntolithError::query("predicate must be an IRI")),
        };
        let object = match &p.object {
            TermPattern::Iri(i) => Term::Iri(i.clone()),
            TermPattern::Literal(l) => Term::Literal(l.clone()),
            TermPattern::Node(n) => Term::BlankNode(*n),
            TermPattern::Variable(v) | TermPattern::Blank(v) => match sol.get(v) {
                Some(BoundValue::Iri(i)) => Term::Iri(i.clone()),
                Some(BoundValue::Literal(l)) => Term::Literal(l.clone()),
                Some(BoundValue::Node(n) | BoundValue::Blank(n)) => Term::BlankNode(*n),
                None => continue,
            },
        };
        out.push(Triple {
            subject,
            predicate,
            object,
        });
    }
    Ok(out)
}

fn triple_key(t: &Triple) -> String {
    format!("{}|{}|{t:?}", t.subject.get(), t.predicate.as_str())
}

/// Read view exposing one named graph's quads as the default graph, used by
/// `WITH <g>` so the WHERE clause matches against graph `g` while IRI→NodeId
/// dictionary lookups still delegate to the base read service.
struct GraphScopedRead<'a> {
    base: &'a dyn QueryReadService,
    triples: Vec<Triple>,
}

impl<'a> GraphScopedRead<'a> {
    fn new(base: &'a dyn QueryReadService, write: &dyn UpdateWriteService, graph: &Iri) -> Self {
        let triples = write
            .quads_in_graph(graph, None)
            .into_iter()
            .map(|q| q.triple)
            .collect();
        Self { base, triples }
    }
}

impl QueryReadService for GraphScopedRead<'_> {
    fn all_triples(&self, _txn_id: Option<TxnId>) -> Result<Vec<Triple>, OntolithError> {
        Ok(self.triples.clone())
    }

    fn by_subject(
        &self,
        subject: NodeId,
        _txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        Ok(self
            .triples
            .iter()
            .filter(|t| t.subject == subject)
            .cloned()
            .collect())
    }

    fn by_predicate(
        &self,
        predicate: &Iri,
        _txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        Ok(self
            .triples
            .iter()
            .filter(|t| &t.predicate == predicate)
            .cloned()
            .collect())
    }

    fn by_object(
        &self,
        object: &Term,
        _txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        Ok(self
            .triples
            .iter()
            .filter(|t| &t.object == object)
            .cloned()
            .collect())
    }

    fn node_for_iri(&self, iri: &Iri) -> Result<Option<NodeId>, OntolithError> {
        self.base.node_for_iri(iri)
    }

    fn encode_node(&self, value: &str) -> Option<NodeId> {
        self.base.encode_node(value)
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
        Algebra::Minus { left, right } => {
            let l = eval_algebra(left, ctx)?;
            let r = eval_algebra(right, ctx)?;
            Ok(minus_join(l, r, ctx))
        }
        Algebra::Filter { expression, input } => {
            let rows = eval_algebra(input, ctx)?;
            let mut out = Vec::new();
            for s in rows {
                ctx.check()?;
                if eval_expr_bool(expression, &s, ctx).unwrap_or(false) {
                    out.push(s);
                }
            }
            Ok(out)
        }
        Algebra::Extend {
            variable,
            expression,
            input,
        } => {
            let mut rows = eval_algebra(input, ctx)?;
            for s in &mut rows {
                ctx.check()?;
                if let Some(v) = eval_expr_value(expression, s, ctx) {
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
                ctx.check()?;
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
            groups,
            aggregates,
            having,
            input,
        } => eval_aggregate(groups, aggregates, having, input, ctx),
        Algebra::Path {
            subject,
            path,
            object,
        } => eval_path_pattern(subject, path, object, ctx),
    }
}

/// SPARQL MINUS: a left row survives unless some right row shares all of its
/// bound variables with identical values (unbound variables never match).
fn minus_join(left: Vec<Solution>, right: Vec<Solution>, ctx: &ExecCtx<'_>) -> Vec<Solution> {
    let mut out = Vec::new();
    for l in &left {
        ctx.check().ok();
        let mut remove = false;
        for r in &right {
            let shared: Vec<&String> = l
                .bindings
                .keys()
                .filter(|k| r.bindings.contains_key(*k))
                .collect();
            if shared.is_empty() {
                continue;
            }
            if shared
                .iter()
                .all(|k| l.bindings.get(*k) == r.bindings.get(*k))
            {
                remove = true;
                break;
            }
        }
        if !remove {
            out.push(l.clone());
        }
    }
    out
}

fn eval_aggregate(
    groups: &[String],
    aggregates: &[AggregateSpec],
    having: &Option<Expression>,
    input: &Algebra,
    ctx: &ExecCtx<'_>,
) -> Result<Vec<Solution>, OntolithError> {
    let rows = eval_algebra(input, ctx)?;

    let mut grouped: BTreeMap<Vec<String>, Vec<Solution>> = BTreeMap::new();
    for row in &rows {
        let key: Vec<String> = groups
            .iter()
            .map(|g| match row.get(g) {
                Some(v) => format!("{v:?}"),
                None => String::new(),
            })
            .collect();
        grouped.entry(key).or_default().push(row.clone());
    }

    let mut out_rows = Vec::new();
    if groups.is_empty() {
        let mut out = Solution::new();
        for spec in aggregates {
            if let Some(v) = eval_aggregate_spec(spec, &rows) {
                out.insert(spec.output.clone(), v);
            }
        }
        out_rows.push(out);
    } else {
        for (_key, group_rows) in grouped {
            let representative = &group_rows[0];
            let mut out = Solution::new();
            for g in groups {
                if let Some(v) = representative.get(g) {
                    out.insert(g.clone(), v.clone());
                }
            }
            for spec in aggregates {
                if let Some(v) = eval_aggregate_spec(spec, &group_rows) {
                    out.insert(spec.output.clone(), v);
                }
            }
            out_rows.push(out);
        }
    }

    if let Some(having) = having {
        out_rows.retain(|row| eval_expr_bool(having, row, ctx) == Some(true));
    }

    Ok(out_rows)
}

fn eval_aggregate_spec(spec: &AggregateSpec, rows: &[Solution]) -> Option<BoundValue> {
    match &spec.function {
        AggregateFunction::Count { variable, distinct } => {
            let n = match (variable.as_deref(), *distinct) {
                (None, _) => rows.len(),
                (Some(v), false) => rows.iter().filter(|s| s.get(v).is_some()).count(),
                (Some(v), true) => {
                    let mut seen = HashSet::new();
                    rows.iter()
                        .filter_map(|s| s.get(v))
                        .filter(|bv| seen.insert(format!("{bv:?}")))
                        .count()
                }
            };
            Some(BoundValue::Literal(LiteralValue::Integer(n as i64)))
        }
        AggregateFunction::Sum { variable, distinct } => {
            let rows = if *distinct {
                distinct_rows(rows, variable)
            } else {
                rows.to_vec()
            };
            let mut acc = 0.0f64;
            let mut acc_i = 0i64;
            let mut rank = 0u8; // 0=int, 1=decimal, 2=float, 3=double
            let mut any = false;
            for s in &rows {
                let Some(bv) = s.get(variable) else { continue };
                let Some(n) = Numeric::from_bound(bv) else {
                    continue;
                };
                rank = rank.max(n.rank());
                acc += n.as_f64();
                if let Numeric::Integer(x) = n {
                    acc_i += x;
                }
                any = true;
            }
            if !any {
                None
            } else if rank == 0 {
                Some(BoundValue::Literal(LiteralValue::Integer(acc_i)))
            } else if rank == 2 {
                Some(BoundValue::Literal(LiteralValue::Float(acc as f32)))
            } else if rank == 3 {
                Some(BoundValue::Literal(LiteralValue::Double(acc)))
            } else {
                Some(BoundValue::Literal(LiteralValue::Decimal(acc)))
            }
        }
        AggregateFunction::Avg { variable, distinct } => {
            let rows = if *distinct {
                distinct_rows(rows, variable)
            } else {
                rows.to_vec()
            };
            let mut acc = 0.0f64;
            let mut n: i64 = 0;
            let mut rank = 0u8;
            for s in &rows {
                if let Some(bv) = s.get(variable)
                    && let Some(x) = Numeric::from_bound(bv)
                {
                    rank = rank.max(x.rank());
                    acc += x.as_f64();
                    n += 1;
                }
            }
            if n == 0 {
                None
            } else if rank == 3 {
                Some(BoundValue::Literal(LiteralValue::Double(acc / n as f64)))
            } else if rank == 2 {
                Some(BoundValue::Literal(LiteralValue::Float(
                    (acc / n as f64) as f32,
                )))
            } else {
                Some(BoundValue::Literal(LiteralValue::Decimal(acc / n as f64)))
            }
        }
        AggregateFunction::Min { variable, distinct } => {
            let rows = if *distinct {
                distinct_rows(rows, variable)
            } else {
                rows.to_vec()
            };
            extremum(variable, &rows, -1)
        }
        AggregateFunction::Max { variable, distinct } => {
            let rows = if *distinct {
                distinct_rows(rows, variable)
            } else {
                rows.to_vec()
            };
            extremum(variable, &rows, 1)
        }
        AggregateFunction::GroupConcat {
            variable,
            separator,
        } => {
            let mut parts = Vec::new();
            let mut lang: Option<String> = None;
            let mut lang_conflict = false;
            for s in rows {
                if let Some(bv) = s.get(variable) {
                    match bv {
                        BoundValue::Literal(LiteralValue::Lang { value, lang: tag }) => {
                            parts.push(value.clone());
                            match &lang {
                                None => lang = Some(tag.as_str().to_owned()),
                                Some(existing) if existing == tag.as_str() => {}
                                Some(_) => lang_conflict = true,
                            }
                        }
                        BoundValue::Literal(LiteralValue::String(v)) => {
                            parts.push(v.clone());
                            if lang.is_none() {
                                lang = Some(String::new());
                            }
                        }
                        _ => return None,
                    }
                }
            }
            if parts.is_empty() {
                None
            } else if !lang_conflict && lang.as_deref().is_some_and(|l| !l.is_empty()) {
                let joined = parts.join(separator);
                Some(BoundValue::Literal(LiteralValue::Lang {
                    value: joined,
                    lang: LanguageTag::parse(lang.unwrap()).ok()?,
                }))
            } else {
                Some(BoundValue::Literal(LiteralValue::String(
                    parts.join(separator),
                )))
            }
        }
        AggregateFunction::Sample { variable } => {
            rows.iter().find_map(|s| s.get(variable).cloned())
        }
    }
}

fn distinct_rows(rows: &[Solution], variable: &str) -> Vec<Solution> {
    let mut seen = HashSet::new();
    rows.iter()
        .filter(|s| s.get(variable).is_some())
        .filter(|s| seen.insert(format!("{:?}", s.get(variable).unwrap())))
        .cloned()
        .collect()
}

/// `sign` of -1 keeps the smaller value (MIN), 1 keeps the larger (MAX).
fn extremum(variable: &str, rows: &[Solution], sign: i8) -> Option<BoundValue> {
    let mut best: Option<BoundValue> = None;
    for s in rows {
        let Some(bv) = s.get(variable) else { continue };
        best = Some(match best {
            None => bv.clone(),
            Some(cur) => match compare_values(bv, &cur) {
                Some(c) if c == sign => bv.clone(),
                _ => cur,
            },
        });
    }
    best
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
        PathExpression::ZeroOrOne(inner) => {
            let mut out = vec![start.clone()];
            let mut seen = HashSet::new();
            seen.insert(path_value_key(start));
            for value in eval_path_from_value(inner, start, ctx)? {
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
            BoundValue::Iri(actual) => Ok(ctx
                .read
                .node_for_iri(actual)?
                .is_some_and(|n| n == *expected)),
            BoundValue::Literal(_) => Ok(false),
        },
        TermPattern::Iri(expected) => match value {
            BoundValue::Iri(actual) => Ok(actual == expected),
            BoundValue::Node(actual) | BoundValue::Blank(actual) => Ok(ctx
                .read
                .node_for_iri(expected)?
                .is_some_and(|n| n == *actual)),
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
                ctx.check()?;
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
            BoundValue::Node(n) | BoundValue::Blank(n) => match ctx.read.node_for_iri(i)? {
                Some(pid) if pid == n => Ok(Some(())),
                _ => Ok(None),
            },
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
        | (BoundValue::Node(node) | BoundValue::Blank(node), BoundValue::Iri(iri)) => Ok(ctx
            .read
            .node_for_iri(iri)?
            .is_some_and(|mapped| mapped == *node)),
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
            ctx.check()?;
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
            ctx.check()?;
            if let Some(m) = merge_solutions_compatible(l, r, ctx)? {
                let ok = condition
                    .map(|c| eval_expr_bool(c, &m, ctx).unwrap_or(false))
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
        (
            BoundValue::Node(a) | BoundValue::Blank(a),
            BoundValue::Node(b) | BoundValue::Blank(b),
        ) => Ok(a == b),
        _ => iri_node_compatible(left, right, ctx),
    }
}

fn eval_expr_bool(expr: &Expression, sol: &Solution, ctx: &ExecCtx<'_>) -> Option<bool> {
    match expr {
        Expression::Bound(v) => Some(sol.get(v).is_some()),
        Expression::Not(e) => Some(!eval_expr_bool(e, sol, ctx)?),
        Expression::And(a, b) => Some(eval_expr_bool(a, sol, ctx)? && eval_expr_bool(b, sol, ctx)?),
        Expression::Or(a, b) => Some(eval_expr_bool(a, sol, ctx)? || eval_expr_bool(b, sol, ctx)?),
        Expression::Equal(a, b) => value_equal(
            &eval_expr_value(a, sol, ctx)?,
            &eval_expr_value(b, sol, ctx)?,
        ),
        Expression::NotEqual(a, b) => value_equal(
            &eval_expr_value(a, sol, ctx)?,
            &eval_expr_value(b, sol, ctx)?,
        )
        .map(|e| !e),
        Expression::Less(a, b) => Some(
            compare_values(
                &eval_expr_value(a, sol, ctx)?,
                &eval_expr_value(b, sol, ctx)?,
            )? < 0,
        ),
        Expression::LessEq(a, b) => Some(
            compare_values(
                &eval_expr_value(a, sol, ctx)?,
                &eval_expr_value(b, sol, ctx)?,
            )? <= 0,
        ),
        Expression::Greater(a, b) => Some(
            compare_values(
                &eval_expr_value(a, sol, ctx)?,
                &eval_expr_value(b, sol, ctx)?,
            )? > 0,
        ),
        Expression::GreaterEq(a, b) => Some(
            compare_values(
                &eval_expr_value(a, sol, ctx)?,
                &eval_expr_value(b, sol, ctx)?,
            )? >= 0,
        ),
        Expression::Negate(e) => match eval_expr_value(e, sol, ctx)? {
            BoundValue::Literal(LiteralValue::Integer(n)) => Some(n != 0),
            BoundValue::Literal(LiteralValue::Decimal(f)) => Some(f != 0.0),
            BoundValue::Literal(LiteralValue::Float(f)) => Some(f != 0.0),
            BoundValue::Literal(LiteralValue::Double(f)) => Some(f != 0.0),
            _ => None,
        },
        Expression::Exists { negated, pattern } => {
            let found = exists_in_solution(pattern, sol, ctx).unwrap_or(false);
            Some(if *negated { !found } else { found })
        }
        Expression::Arith { op, left, right } => {
            let l = eval_expr_value(left, sol, ctx)?;
            let r = eval_expr_value(right, sol, ctx)?;
            let v = arithmetic(*op, &l, &r)?;
            match v {
                BoundValue::Literal(LiteralValue::Integer(n)) => Some(n != 0),
                BoundValue::Literal(LiteralValue::Decimal(f)) => Some(f != 0.0),
                BoundValue::Literal(LiteralValue::Float(f)) => Some(f != 0.0),
                BoundValue::Literal(LiteralValue::Double(f)) => Some(f != 0.0),
                _ => None,
            }
        }
        Expression::Function { .. } => match eval_expr_value(expr, sol, ctx)? {
            BoundValue::Literal(LiteralValue::Boolean(b)) => Some(b),
            _ => None,
        },
        Expression::IsIri(e) => Some(matches!(eval_expr_value(e, sol, ctx)?, BoundValue::Iri(_))),
        Expression::IsLiteral(e) => Some(matches!(
            eval_expr_value(e, sol, ctx)?,
            BoundValue::Literal(_)
        )),
        Expression::IsBlank(e) => Some(matches!(
            eval_expr_value(e, sol, ctx)?,
            BoundValue::Blank(_) | BoundValue::Node(_)
        )),
        Expression::Variable(v) => sol.get(v).map(|_| true),
        Expression::Literal(LiteralValue::Boolean(b)) => Some(*b),
        _ => eval_expr_value(expr, sol, ctx).map(|_| true),
    }
}

fn eval_expr_value(expr: &Expression, sol: &Solution, ctx: &ExecCtx<'_>) -> Option<BoundValue> {
    match expr {
        Expression::Variable(v) => sol.get(v).cloned(),
        Expression::Iri(i) => Some(BoundValue::Iri(i.clone())),
        Expression::Literal(l) => Some(BoundValue::Literal(l.clone())),
        Expression::Bound(v) => Some(BoundValue::Literal(LiteralValue::Boolean(
            sol.get(v).is_some(),
        ))),
        Expression::Not(e) => Some(BoundValue::Literal(LiteralValue::Boolean(!eval_expr_bool(
            e, sol, ctx,
        )?))),
        Expression::And(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            eval_expr_bool(a, sol, ctx)? && eval_expr_bool(b, sol, ctx)?,
        ))),
        Expression::Or(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            eval_expr_bool(a, sol, ctx)? || eval_expr_bool(b, sol, ctx)?,
        ))),
        Expression::Equal(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(value_equal(
            &eval_expr_value(a, sol, ctx)?,
            &eval_expr_value(b, sol, ctx)?,
        )?))),
        Expression::NotEqual(a, b) => {
            Some(BoundValue::Literal(LiteralValue::Boolean(!value_equal(
                &eval_expr_value(a, sol, ctx)?,
                &eval_expr_value(b, sol, ctx)?,
            )?)))
        }
        Expression::IsIri(e) => Some(BoundValue::Literal(LiteralValue::Boolean(matches!(
            eval_expr_value(e, sol, ctx)?,
            BoundValue::Iri(_)
        )))),
        Expression::IsLiteral(e) => Some(BoundValue::Literal(LiteralValue::Boolean(matches!(
            eval_expr_value(e, sol, ctx)?,
            BoundValue::Literal(_)
        )))),
        Expression::IsBlank(e) => Some(BoundValue::Literal(LiteralValue::Boolean(matches!(
            eval_expr_value(e, sol, ctx)?,
            BoundValue::Blank(_) | BoundValue::Node(_)
        )))),
        Expression::Less(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            compare_values(
                &eval_expr_value(a, sol, ctx)?,
                &eval_expr_value(b, sol, ctx)?,
            )? < 0,
        ))),
        Expression::LessEq(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            compare_values(
                &eval_expr_value(a, sol, ctx)?,
                &eval_expr_value(b, sol, ctx)?,
            )? <= 0,
        ))),
        Expression::Greater(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            compare_values(
                &eval_expr_value(a, sol, ctx)?,
                &eval_expr_value(b, sol, ctx)?,
            )? > 0,
        ))),
        Expression::GreaterEq(a, b) => Some(BoundValue::Literal(LiteralValue::Boolean(
            compare_values(
                &eval_expr_value(a, sol, ctx)?,
                &eval_expr_value(b, sol, ctx)?,
            )? >= 0,
        ))),
        Expression::Negate(e) => match eval_expr_value(e, sol, ctx)? {
            BoundValue::Literal(LiteralValue::Integer(n)) => {
                Some(BoundValue::Literal(LiteralValue::Integer(-n)))
            }
            BoundValue::Literal(LiteralValue::Decimal(f)) => {
                Some(BoundValue::Literal(LiteralValue::Decimal(-f)))
            }
            BoundValue::Literal(LiteralValue::Float(f)) => {
                Some(BoundValue::Literal(LiteralValue::Float(-f)))
            }
            BoundValue::Literal(LiteralValue::Double(f)) => {
                Some(BoundValue::Literal(LiteralValue::Double(-f)))
            }
            _ => None,
        },
        Expression::Exists { .. } => Some(BoundValue::Literal(LiteralValue::Boolean(
            eval_expr_bool(expr, sol, ctx)?,
        ))),
        Expression::Arith { op, left, right } => {
            let l = eval_expr_value(left, sol, ctx)?;
            let r = eval_expr_value(right, sol, ctx)?;
            arithmetic(*op, &l, &r)
        }
        Expression::Function { name, args } => eval_function(name, args, sol, ctx),
        // Aggregates are only valid in HAVING and are rewritten to their
        // projection alias before execution; a stray call is an error.
        Expression::Aggregate(_) => None,
    }
}

/// Built-in SPARQL function subset over the compact literal model (P3-05).
fn eval_function(
    name: &str,
    args: &[Expression],
    sol: &Solution,
    ctx: &ExecCtx<'_>,
) -> Option<BoundValue> {
    let arg = |i: usize| eval_expr_value(args.get(i)?, sol, ctx);
    // STR accepts IRIs; other string functions require plain/lang/string literals.
    let string_of = |bv: &BoundValue| -> Option<String> {
        match bv {
            BoundValue::Literal(l) => Some(l.lexical_form()),
            BoundValue::Iri(i) => Some(i.as_str().to_owned()),
            _ => None,
        }
    };
    let string = |i: usize| string_of(&arg(i)?);
    let str_lit = |i: usize| -> Option<LiteralValue> {
        let bv = arg(i)?;
        match &bv {
            BoundValue::Literal(l) => str_lit_lex(l).map(|_| l.clone()),
            _ => None,
        }
    };
    let int_of =
        |bv: &BoundValue| -> Option<i64> { Numeric::from_bound(bv).map(|n| n.as_f64() as i64) };
    let bool = |b: bool| BoundValue::Literal(LiteralValue::Boolean(b));
    let strlit = |s: String| BoundValue::Literal(LiteralValue::String(s));

    match name {
        "CAST" => {
            let datatype = match arg(1)? {
                BoundValue::Iri(i) => i,
                _ => return None,
            };
            cast_value(&arg(0)?, &datatype, &strlit)
        }
        _ if name.starts_with("XSD:") => {
            let suffix = name[4..].to_ascii_lowercase();
            let datatype = Iri::new(format!("http://www.w3.org/2001/XMLSchema#{suffix}"));
            cast_value(&arg(0)?, &datatype, &strlit)
        }
        "STR" => match arg(0)? {
            BoundValue::Iri(i) => Some(strlit(i.as_str().to_owned())),
            BoundValue::Literal(l) => Some(strlit(l.lexical_form())),
            _ => None,
        },
        "UCASE" => {
            let l = str_lit(0)?;
            Some(BoundValue::Literal(str_result(
                &l,
                str_lit_lex(&l)?.to_uppercase(),
            )))
        }
        "LCASE" => {
            let l = str_lit(0)?;
            Some(BoundValue::Literal(str_result(
                &l,
                str_lit_lex(&l)?.to_lowercase(),
            )))
        }
        "STRLEN" => Some(BoundValue::Literal(LiteralValue::Integer(
            str_lit_lex(&str_lit(0)?)?.chars().count() as i64,
        ))),
        "CONCAT" => {
            let mut out = String::new();
            let mut lang: Option<String> = None;
            let mut all_lang = true;
            for i in 0..args.len() {
                let l = str_lit(i)?;
                out.push_str(str_lit_lex(&l)?);
                match l {
                    LiteralValue::Lang { lang: tag, .. } => match &lang {
                        None => lang = Some(tag.as_str().to_owned()),
                        Some(existing) if existing != tag.as_str() => lang = None,
                        Some(_) => {}
                    },
                    _ => all_lang = false,
                }
            }
            let value = if all_lang && let Some(tag) = lang {
                BoundValue::Literal(LiteralValue::Lang {
                    value: out,
                    lang: LanguageTag::parse(tag).ok()?,
                })
            } else {
                strlit(out)
            };
            Some(value)
        }
        "SUBSTR" => {
            let l = str_lit(0)?;
            let s = str_lit_lex(&l)?.to_owned();
            let start = int_of(&arg(1)?)?;
            let chars: Vec<char> = s.chars().collect();
            let from = (start.max(1) as usize - 1).min(chars.len());
            let out: String = if args.len() >= 3 {
                let len = int_of(&arg(2)?)?.max(0) as usize;
                chars.iter().skip(from).take(len).collect()
            } else {
                chars.iter().skip(from).collect()
            };
            Some(BoundValue::Literal(str_result(&l, out)))
        }
        "CONTAINS" => {
            let a = str_lit_lex(&str_lit(0)?)?.to_owned();
            let b = str_lit_lex(&str_lit(1)?)?.to_owned();
            Some(bool(a.contains(&b)))
        }
        "MD5" | "SHA1" | "SHA256" | "SHA384" | "SHA512" => {
            let l = str_lit(0)?;
            let s = str_lit_lex(&l)?;
            let hex_digest = match name {
                "MD5" => super::hashes::hex(&super::hashes::md5(s.as_bytes())),
                "SHA1" => super::hashes::hex(&super::hashes::sha1(s.as_bytes())),
                "SHA256" => super::hashes::hex(&super::hashes::sha256(s.as_bytes())),
                "SHA384" => super::hashes::hex(&super::hashes::sha384(s.as_bytes())),
                _ => super::hashes::hex(&super::hashes::sha512(s.as_bytes())),
            };
            Some(strlit(hex_digest))
        }
        "STRSTARTS" => {
            let a = str_lit_lex(&str_lit(0)?)?.to_owned();
            let b = str_lit_lex(&str_lit(1)?)?.to_owned();
            Some(bool(a.starts_with(&b)))
        }
        "STRENDS" => {
            let a = str_lit_lex(&str_lit(0)?)?.to_owned();
            let b = str_lit_lex(&str_lit(1)?)?.to_owned();
            Some(bool(a.ends_with(&b)))
        }
        "STRAFTER" => {
            let l = str_lit(0)?;
            let needle_lit = str_lit(1)?;
            if !str_args_compatible(&l, &needle_lit) {
                return None;
            }
            let s = str_lit_lex(&l)?.to_owned();
            let needle = str_lit_lex(&needle_lit)?.to_owned();
            match s.find(&needle) {
                Some(idx) => Some(BoundValue::Literal(str_result(
                    &l,
                    s[idx + needle.len()..].to_owned(),
                ))),
                None => Some(strlit(String::new())),
            }
        }
        "STRBEFORE" => {
            let l = str_lit(0)?;
            let needle_lit = str_lit(1)?;
            if !str_args_compatible(&l, &needle_lit) {
                return None;
            }
            let s = str_lit_lex(&l)?.to_owned();
            let needle = str_lit_lex(&needle_lit)?.to_owned();
            match s.find(&needle) {
                Some(idx) => Some(BoundValue::Literal(str_result(&l, s[..idx].to_owned()))),
                None => Some(strlit(String::new())),
            }
        }
        "LANG" => match arg(0)? {
            BoundValue::Literal(LiteralValue::Lang { lang, .. }) => {
                Some(strlit(lang.as_str().to_owned()))
            }
            BoundValue::Literal(_) => Some(strlit(String::new())),
            _ => None,
        },
        "LANGMATCHES" => {
            let tag = str_lit_lex(&str_lit(0)?)?.to_ascii_lowercase();
            let range = str_lit_lex(&str_lit(1)?)?.to_ascii_lowercase();
            Some(bool(lang_matches(&tag, &range)))
        }
        "DATATYPE" => match arg(0)? {
            BoundValue::Literal(l) => Some(BoundValue::Iri(l.xsd_datatype_iri())),
            _ => None,
        },
        "IRI" | "URI" => {
            let s = string(0)?;
            let resolved = match ctx.base {
                Some(base) => resolve_iri(&s, base),
                None => Some(s),
            }?;
            Iri::parse(resolved).ok().map(BoundValue::Iri)
        }
        "STRDT" => {
            let lit = str_lit(0)?;
            if matches!(lit, LiteralValue::Lang { .. }) {
                return None;
            }
            let datatype = match arg(1)? {
                BoundValue::Iri(i) => i,
                _ => return None,
            };
            Some(BoundValue::Literal(LiteralValue::Typed {
                value: str_lit_lex(&lit)?.to_owned(),
                datatype,
            }))
        }
        "STRLANG" => {
            let lit = str_lit(0)?;
            if matches!(lit, LiteralValue::Lang { .. }) {
                return None;
            }
            let lang = match arg(1)? {
                BoundValue::Literal(LiteralValue::String(s)) => LanguageTag::parse(s).ok()?,
                _ => return None,
            };
            Some(BoundValue::Literal(LiteralValue::Lang {
                value: str_lit_lex(&lit)?.to_owned(),
                lang,
            }))
        }
        "ABS" => {
            let n = Numeric::from_bound(&arg(0)?)?;
            let v = match n {
                Numeric::Integer(x) => LiteralValue::Integer(x.abs()),
                Numeric::Decimal(x) => LiteralValue::Decimal(x.abs()),
                Numeric::Float(x) => LiteralValue::Float(x.abs()),
                Numeric::Double(x) => LiteralValue::Double(x.abs()),
            };
            Some(BoundValue::Literal(v))
        }
        "CEIL" => numeric_unary(&arg(0)?, |f| f.ceil(), |f| f.ceil(), |f| f.ceil() as i64),
        "FLOOR" => numeric_unary(&arg(0)?, |f| f.floor(), |f| f.floor(), |f| f.floor() as i64),
        "ROUND" => numeric_unary(&arg(0)?, |f| f.round(), |f| f.round(), |f| f.round() as i64),
        "ISNUMERIC" => Some(bool(Numeric::from_bound(&arg(0)?).is_some())),
        "IF" => {
            let cond = eval_expr_bool(&args[0], sol, ctx)?;
            if cond {
                eval_expr_value(args.get(1)?, sol, ctx)
            } else {
                eval_expr_value(args.get(2)?, sol, ctx)
            }
        }
        "COALESCE" => {
            for a in args {
                if let Some(v) = eval_expr_value(a, sol, ctx) {
                    return Some(v);
                }
            }
            None
        }
        "IN" | "NOT IN" => {
            let needle = eval_expr_value(args.first()?, sol, ctx)?;
            let mut found = false;
            let mut error = false;
            for a in args.iter().skip(1) {
                let Some(v) = eval_expr_value(a, sol, ctx) else {
                    error = true;
                    continue;
                };
                match value_equal(&v, &needle) {
                    Some(true) => {
                        found = true;
                        break;
                    }
                    Some(false) => {}
                    None => error = true,
                }
            }
            if !found && error {
                return None;
            }
            Some(bool(if name == "IN" { found } else { !found }))
        }
        "ENCODE_FOR_URI" => Some(strlit(encode_for_uri(&string(0)?))),
        "NOW" => Some(BoundValue::Literal(LiteralValue::Typed {
            value: now_datetime(),
            datatype: Iri::new("http://www.w3.org/2001/XMLSchema#dateTime"),
        })),
        "RAND" => {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .subsec_nanos();
            let x = (nanos as f64) / 1_000_000_000_f64;
            Some(BoundValue::Literal(LiteralValue::Double(x)))
        }
        "YEAR" | "MONTH" | "DAY" | "HOURS" | "MINUTES" | "SECONDS" => {
            let dt = datetime_of(&arg(0)?)?;
            let part = match name {
                "YEAR" => dt.year,
                "MONTH" => dt.month,
                "DAY" => dt.day,
                "HOURS" => dt.hour,
                "MINUTES" => dt.minute,
                _ => dt.second_trunc,
            };
            let v = if name == "SECONDS" {
                LiteralValue::Decimal(part as f64)
            } else {
                LiteralValue::Integer(part)
            };
            Some(BoundValue::Literal(v))
        }
        "TIMEZONE" => {
            let dt = datetime_of(&arg(0)?)?;
            if matches!(dt.tz, TimezoneOffset::None) {
                return None;
            }
            let lex = format_duration(&dt.tz);
            Some(BoundValue::Literal(LiteralValue::Typed {
                value: lex,
                datatype: Iri::new("http://www.w3.org/2001/XMLSchema#dayTimeDuration"),
            }))
        }
        "TZ" => {
            let dt = datetime_of(&arg(0)?)?;
            Some(strlit(dt.tz.to_string()))
        }
        _ => None,
    }
}

/// Lexical string of a string-usable literal (plain / lang / xsd:string).
fn str_lit_lex(l: &LiteralValue) -> Option<&str> {
    match l {
        LiteralValue::String(s) => Some(s),
        LiteralValue::Lang { value, .. } => Some(value),
        LiteralValue::Typed { value, datatype } if datatype.as_str() == XSD_STRING_IRI => {
            Some(value)
        }
        _ => None,
    }
}

/// SPARQL argument compatibility for STRBEFORE/STRAFTER: both simple or
/// xsd:string; both plain literals with the same language tag; or left is a
/// plain literal with a language tag and right is simple or xsd:string.
fn str_args_compatible(l: &LiteralValue, r: &LiteralValue) -> bool {
    let simple = |lit: &LiteralValue| match lit {
        LiteralValue::String(_) => true,
        LiteralValue::Typed { datatype, .. } if datatype.as_str() == XSD_STRING_IRI => true,
        _ => false,
    };
    match (l, r) {
        (LiteralValue::Lang { lang: a, .. }, LiteralValue::Lang { lang: b, .. }) => a == b,
        (LiteralValue::Lang { .. }, _) => simple(r),
        (_, _) => simple(l) && simple(r),
    }
}

/// Result literal for string-preserving functions (lang carried over).
fn str_result(orig: &LiteralValue, lex: String) -> LiteralValue {
    match orig {
        LiteralValue::Lang { lang, .. } => LiteralValue::Lang {
            value: lex,
            lang: lang.clone(),
        },
        _ => LiteralValue::String(lex),
    }
}

/// RFC 4647 basic filtering: `*` matches any tag; otherwise the tag equals
/// the range or begins with `range-`.
fn lang_matches(tag: &str, range: &str) -> bool {
    if range == "*" {
        return !tag.is_empty();
    }
    tag == range
        || tag
            .strip_prefix(range)
            .is_some_and(|rest| rest.starts_with('-'))
}

/// CEIL/FLOOR/ROUND preserve the numeric datatype of the argument.
fn numeric_unary(
    bv: &BoundValue,
    d: impl Fn(f64) -> f64,
    f: impl Fn(f32) -> f32,
    i: impl Fn(f64) -> i64,
) -> Option<BoundValue> {
    match Numeric::from_bound(bv)? {
        Numeric::Integer(x) => Some(BoundValue::Literal(LiteralValue::Integer(i(x as f64)))),
        Numeric::Decimal(x) => Some(BoundValue::Literal(LiteralValue::Decimal(d(x)))),
        Numeric::Float(x) => Some(BoundValue::Literal(LiteralValue::Float(f(x)))),
        Numeric::Double(x) => Some(BoundValue::Literal(LiteralValue::Double(d(x)))),
    }
}

fn encode_for_uri(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Resolve a (possibly relative) IRI reference against a base IRI (RFC 3986
/// §5.3 subset: absolute, network-path, root-relative, and relative refs).
fn resolve_iri(reference: &str, base: &str) -> Option<String> {
    if reference.contains(':') {
        return Some(reference.to_owned());
    }
    if base.is_empty() {
        return None;
    }
    let (scheme, rest) = base.split_once(':')?;
    let no_fragment = rest.split('#').next().unwrap_or(rest);
    let (authority, path) = match no_fragment.strip_prefix("//") {
        Some(a) => match a.find('/') {
            Some(idx) => (Some(&a[..idx]), &a[idx..]),
            None => (Some(a), ""),
        },
        None => (None, no_fragment),
    };
    let path = match path.find(['?', '#']) {
        Some(idx) => &path[..idx],
        None => path,
    };
    let prefix = match authority {
        Some(a) => format!("{scheme}://{a}"),
        None => format!("{scheme}:"),
    };
    if reference.starts_with("//") {
        return Some(format!("{scheme}:{reference}"));
    }
    if reference.starts_with('/') {
        return Some(format!("{prefix}{}", remove_dot_segments(reference)));
    }
    if reference.starts_with('?') || reference.starts_with('#') {
        return Some(format!("{prefix}{path}{reference}"));
    }
    let dir = match path.rfind('/') {
        Some(idx) => &path[..=idx],
        None => "",
    };
    Some(format!(
        "{prefix}{}",
        remove_dot_segments(&format!("{dir}{reference}"))
    ))
}

fn remove_dot_segments(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "." => {}
            ".." => {
                out.pop();
            }
            "" if out.is_empty() => {}
            other => out.push(other),
        }
    }
    let joined = out.join("/");
    if path.starts_with('/') && !joined.starts_with('/') {
        format!("/{joined}")
    } else {
        joined
    }
}

struct DateTimeParts {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second_trunc: i64,
    tz: TimezoneOffset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimezoneOffset {
    Z,
    /// (sign, hours, minutes); sign true = '+', false = '-'.
    Offset(bool, u32, u32),
    None,
}

impl std::fmt::Display for TimezoneOffset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Z => f.write_str("Z"),
            Self::Offset(true, h, m) => write!(f, "+{h:02}:{m:02}"),
            Self::Offset(false, h, m) => write!(f, "-{h:02}:{m:02}"),
            Self::None => Ok(()),
        }
    }
}

fn format_duration(tz: &TimezoneOffset) -> String {
    match tz {
        TimezoneOffset::Z | TimezoneOffset::None => "PT0S".to_owned(),
        TimezoneOffset::Offset(sign, h, m) => {
            let sign = if *sign { "" } else { "-" };
            let mut out = String::new();
            if *h > 0 {
                out.push_str(&format!("{h}H"));
            }
            if *m > 0 {
                out.push_str(&format!("{m}M"));
            }
            if out.is_empty() {
                out.push_str("0S");
            }
            format!("{sign}PT{out}")
        }
    }
}

fn datetime_of(bv: &BoundValue) -> Option<DateTimeParts> {
    let lex = match bv {
        BoundValue::Literal(LiteralValue::Typed { value, datatype })
            if datatype.as_str() == "http://www.w3.org/2001/XMLSchema#dateTime" =>
        {
            value
        }
        _ => return None,
    };
    parse_datetime(lex)
}

fn parse_datetime(s: &str) -> Option<DateTimeParts> {
    // YYYY-MM-DDTHH:MM:SS(.fff)?(Z|±HH:MM)?
    let b = s.as_bytes();
    let mut i = 0;
    let take = |i: &mut usize, n: usize| -> Option<i64> {
        if *i + n > b.len() {
            return None;
        }
        let part = &s[*i..*i + n];
        *i += n;
        part.parse::<i64>().ok()
    };
    let year = take(&mut i, 4)?;
    if b.get(i) != Some(&b'-') {
        return None;
    }
    i += 1;
    let month = take(&mut i, 2)?;
    if b.get(i) != Some(&b'-') {
        return None;
    }
    i += 1;
    let day = take(&mut i, 2)?;
    if b.get(i) != Some(&b'T') {
        return None;
    }
    i += 1;
    let hour = take(&mut i, 2)?;
    if b.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    let minute = take(&mut i, 2)?;
    if b.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    let sec_start = i;
    take(&mut i, 2)?;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let frac_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == frac_start {
            return None;
        }
    }
    let second_trunc = s[sec_start..sec_start + 2].parse::<i64>().ok()?;
    let tz = if i >= b.len() {
        TimezoneOffset::None
    } else if b[i] == b'Z' {
        i += 1;
        TimezoneOffset::Z
    } else if matches!(b[i], b'+' | b'-') {
        let sign = b[i] == b'+';
        i += 1;
        let th = take(&mut i, 2)?;
        if b.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        let tm = take(&mut i, 2)?;
        TimezoneOffset::Offset(sign, th as u32, tm as u32)
    } else {
        return None;
    };
    if i != b.len() {
        return None;
    }
    Some(DateTimeParts {
        year,
        month,
        day,
        hour,
        minute,
        second_trunc,
        tz,
    })
}

fn now_datetime() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    // Howard Hinnant's civil_from_days algorithm.
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

const XSD_STRING_IRI: &str = "http://www.w3.org/2001/XMLSchema#string";
const XSD_INTEGER_IRI: &str = "http://www.w3.org/2001/XMLSchema#integer";
const XSD_DECIMAL_IRI: &str = "http://www.w3.org/2001/XMLSchema#decimal";
const XSD_FLOAT_IRI: &str = "http://www.w3.org/2001/XMLSchema#float";
const XSD_DOUBLE_IRI: &str = "http://www.w3.org/2001/XMLSchema#double";
const XSD_BOOLEAN_IRI: &str = "http://www.w3.org/2001/XMLSchema#boolean";

/// SPARQL numeric value for promotion-aware arithmetic / comparison.
#[derive(Debug, Clone, Copy)]
enum Numeric {
    Integer(i64),
    Decimal(f64),
    Float(f32),
    Double(f64),
}

impl Numeric {
    fn from_literal(l: &LiteralValue) -> Option<Self> {
        match l {
            LiteralValue::Integer(x) => Some(Self::Integer(*x)),
            LiteralValue::Decimal(x) => Some(Self::Decimal(*x)),
            LiteralValue::Float(x) => Some(Self::Float(*x)),
            LiteralValue::Double(x) => Some(Self::Double(*x)),
            LiteralValue::Typed { value, datatype } => {
                let dt = datatype.as_str();
                if dt == XSD_INTEGER_IRI {
                    value.parse::<i64>().ok().map(Self::Integer)
                } else if dt == XSD_DECIMAL_IRI {
                    value.parse::<f64>().ok().map(Self::Decimal)
                } else if dt == XSD_FLOAT_IRI {
                    value.parse::<f32>().ok().map(Self::Float)
                } else if dt == XSD_DOUBLE_IRI {
                    value.parse::<f64>().ok().map(Self::Double)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn from_bound(bv: &BoundValue) -> Option<Self> {
        match bv {
            BoundValue::Literal(l) => Self::from_literal(l),
            _ => None,
        }
    }

    fn as_f64(self) -> f64 {
        match self {
            Self::Integer(x) => x as f64,
            Self::Decimal(x) => x,
            Self::Float(x) => x as f64,
            Self::Double(x) => x,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Integer(_) => 0,
            Self::Decimal(_) => 1,
            Self::Float(_) => 2,
            Self::Double(_) => 3,
        }
    }
}

/// SPARQL arithmetic with XPath numeric type promotion.
fn arithmetic(op: char, l: &BoundValue, r: &BoundValue) -> Option<BoundValue> {
    let ln = Numeric::from_bound(l)?;
    let rn = Numeric::from_bound(r)?;
    let rank = ln.rank().max(rn.rank());
    let lf = ln.as_f64();
    let rf = rn.as_f64();
    let value = match op {
        '+' => lf + rf,
        '-' => lf - rf,
        '*' => lf * rf,
        '/' => {
            if rf == 0.0 {
                return None;
            }
            lf / rf
        }
        _ => return None,
    };
    if op == '/' && rank == 0 {
        // XPath op:numeric-divide: integer / integer -> decimal.
        return Some(BoundValue::Literal(LiteralValue::Decimal(value)));
    }
    if rank == 2 {
        let v = match op {
            '+' => (lf as f32) + (rf as f32),
            '-' => (lf as f32) - (rf as f32),
            '*' => (lf as f32) * (rf as f32),
            '/' => (lf as f32) / (rf as f32),
            _ => return None,
        };
        return Some(BoundValue::Literal(LiteralValue::Float(v)));
    }
    Some(match rank {
        0 => BoundValue::Literal(LiteralValue::Integer(value as i64)),
        1 => BoundValue::Literal(LiteralValue::Decimal(value)),
        _ => BoundValue::Literal(LiteralValue::Double(value)),
    })
}

/// SPARQL `=` / `!=` value equality (RDFterm-equal). `None` = evaluation error.
fn value_equal(a: &BoundValue, b: &BoundValue) -> Option<bool> {
    match (a, b) {
        (BoundValue::Iri(x), BoundValue::Iri(y)) => Some(x == y),
        (BoundValue::Iri(_), _) | (_, BoundValue::Iri(_)) => Some(false),
        (
            BoundValue::Blank(x) | BoundValue::Node(x),
            BoundValue::Blank(y) | BoundValue::Node(y),
        ) => Some(x == y),
        (BoundValue::Blank(_) | BoundValue::Node(_), _)
        | (_, BoundValue::Blank(_) | BoundValue::Node(_)) => Some(false),
        (BoundValue::Literal(x), BoundValue::Literal(y)) => literal_equal(x, y),
    }
}

fn literal_equal(a: &LiteralValue, b: &LiteralValue) -> Option<bool> {
    if let (Some(na), Some(nb)) = (Numeric::from_literal(a), Numeric::from_literal(b)) {
        let va = na.as_f64();
        let vb = nb.as_f64();
        if va.is_nan() || vb.is_nan() {
            return None;
        }
        return Some(va == vb);
    }
    if Numeric::from_literal(a).is_some() || Numeric::from_literal(b).is_some() {
        // Numeric vs non-numeric: unequal (SPARQL: no error).
        return Some(false);
    }
    match (a, b) {
        (LiteralValue::Lang { value: x, lang: lx }, LiteralValue::Lang { value: y, lang: ly }) => {
            Some(x == y && lx == ly)
        }
        (LiteralValue::Lang { .. }, _) | (_, LiteralValue::Lang { .. }) => Some(false),
        (LiteralValue::Typed { .. }, _) | (_, LiteralValue::Typed { .. }) => {
            let (x, dx) = match a {
                LiteralValue::Typed { value, datatype } => (value, datatype.as_str()),
                LiteralValue::String(v) => (v, XSD_STRING_IRI),
                _ => return None,
            };
            let (y, dy) = match b {
                LiteralValue::Typed { value, datatype } => (value, datatype.as_str()),
                LiteralValue::String(v) => (v, XSD_STRING_IRI),
                _ => return None,
            };
            Some(x == y && dx == dy)
        }
        (LiteralValue::String(x), LiteralValue::String(y)) => Some(x == y),
        (LiteralValue::Boolean(x), LiteralValue::Boolean(y)) => Some(x == y),
        _ => Some(false),
    }
}

/// SPARQL ordering comparison (`<` `<=` `>` `>=`). `None` = incomparable/error.
fn compare_values(a: &BoundValue, b: &BoundValue) -> Option<i8> {
    if let (Some(na), Some(nb)) = (Numeric::from_bound(a), Numeric::from_bound(b)) {
        let va = na.as_f64();
        let vb = nb.as_f64();
        if va.is_nan() || vb.is_nan() {
            return None;
        }
        return Some(if va < vb {
            -1
        } else if va > vb {
            1
        } else {
            0
        });
    }
    match (a, b) {
        (BoundValue::Iri(x), BoundValue::Iri(y)) => Some(ord(x.as_str().cmp(y.as_str()))),
        (BoundValue::Literal(x), BoundValue::Literal(y)) => {
            let (lx, lang_x) = string_lex_and_lang(x)?;
            let (ly, lang_y) = string_lex_and_lang(y)?;
            let base = ord(lx.cmp(ly));
            if base != 0 {
                Some(base)
            } else {
                // Language-tagged literals sort after plain literals with the
                // same value, then by tag.
                Some(match (lang_x, lang_y) {
                    (Some(a), Some(b)) => ord(a.cmp(b)),
                    (Some(_), None) => 1,
                    (None, Some(_)) => -1,
                    (None, None) => 0,
                })
            }
        }
        _ => None,
    }
}

fn ord(o: std::cmp::Ordering) -> i8 {
    match o {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

fn string_lex_and_lang(l: &LiteralValue) -> Option<(&str, Option<&str>)> {
    match l {
        LiteralValue::String(s) => Some((s, None)),
        LiteralValue::Lang { value, lang } => Some((value, Some(lang.as_str()))),
        LiteralValue::Typed { value, datatype } if datatype.as_str() == XSD_STRING_IRI => {
            Some((value, None))
        }
        _ => None,
    }
}

/// SPARQL `CAST(expr AS datatype)` / `xsd:type(expr)`.
fn cast_value(
    bv: &BoundValue,
    datatype: &Iri,
    strlit: &dyn Fn(String) -> BoundValue,
) -> Option<BoundValue> {
    let dt = datatype.as_str();
    match dt {
        XSD_STRING_IRI => {
            let s = match bv {
                BoundValue::Literal(l) => l.lexical_form(),
                BoundValue::Iri(i) => i.as_str().to_owned(),
                _ => return None,
            };
            Some(strlit(s))
        }
        XSD_BOOLEAN_IRI => {
            let b = match bv {
                BoundValue::Literal(LiteralValue::Boolean(b)) => *b,
                BoundValue::Literal(LiteralValue::Integer(n)) => *n != 0,
                BoundValue::Literal(LiteralValue::Decimal(f)) => *f != 0.0,
                BoundValue::Literal(LiteralValue::Float(f)) => *f != 0.0,
                BoundValue::Literal(LiteralValue::Double(f)) => *f != 0.0,
                BoundValue::Literal(LiteralValue::String(s)) => match s.trim() {
                    "true" | "1" => true,
                    "false" | "0" => false,
                    _ => return None,
                },
                BoundValue::Literal(LiteralValue::Typed { value, datatype })
                    if matches!(
                        datatype.as_str(),
                        XSD_INTEGER_IRI | XSD_DECIMAL_IRI | XSD_FLOAT_IRI | XSD_DOUBLE_IRI
                    ) =>
                {
                    Numeric::from_literal(&LiteralValue::Typed {
                        value: value.clone(),
                        datatype: datatype.clone(),
                    })?
                    .as_f64()
                        != 0.0
                }
                _ => return None,
            };
            Some(BoundValue::Literal(LiteralValue::Boolean(b)))
        }
        XSD_INTEGER_IRI => {
            let n = match bv {
                BoundValue::Literal(LiteralValue::Integer(n)) => *n,
                BoundValue::Literal(LiteralValue::Decimal(f)) => f.trunc() as i64,
                BoundValue::Literal(LiteralValue::Float(f)) => f.trunc() as i64,
                BoundValue::Literal(LiteralValue::Double(f)) => f.trunc() as i64,
                BoundValue::Literal(LiteralValue::Boolean(b)) => *b as i64,
                BoundValue::Literal(LiteralValue::String(s)) if valid_integer_lex(s.trim()) => {
                    s.trim().parse::<i64>().ok()?
                }
                BoundValue::Literal(LiteralValue::Typed { value, datatype })
                    if matches!(
                        datatype.as_str(),
                        XSD_INTEGER_IRI | XSD_DECIMAL_IRI | XSD_FLOAT_IRI | XSD_DOUBLE_IRI
                    ) =>
                {
                    let num = Numeric::from_literal(&LiteralValue::Typed {
                        value: value.clone(),
                        datatype: datatype.clone(),
                    })?;
                    match num {
                        Numeric::Integer(x) => x,
                        other => other.as_f64().trunc() as i64,
                    }
                }
                _ => return None,
            };
            Some(BoundValue::Literal(LiteralValue::Integer(n)))
        }
        XSD_DECIMAL_IRI => {
            let v = match bv {
                BoundValue::Literal(LiteralValue::Integer(n)) => *n as f64,
                BoundValue::Literal(LiteralValue::Decimal(f)) => *f,
                BoundValue::Literal(LiteralValue::Float(f)) => *f as f64,
                BoundValue::Literal(LiteralValue::Double(f)) => *f,
                BoundValue::Literal(LiteralValue::Boolean(b)) => {
                    if *b {
                        1.0
                    } else {
                        0.0
                    }
                }
                BoundValue::Literal(LiteralValue::String(s)) if valid_decimal_lex(s.trim()) => {
                    s.trim().parse::<f64>().ok()?
                }
                BoundValue::Literal(LiteralValue::Typed { value, datatype })
                    if matches!(
                        datatype.as_str(),
                        XSD_INTEGER_IRI | XSD_DECIMAL_IRI | XSD_FLOAT_IRI | XSD_DOUBLE_IRI
                    ) =>
                {
                    let num = Numeric::from_literal(&LiteralValue::Typed {
                        value: value.clone(),
                        datatype: datatype.clone(),
                    })?;
                    num.as_f64()
                }
                _ => return None,
            };
            Some(BoundValue::Literal(LiteralValue::Decimal(v)))
        }
        XSD_DOUBLE_IRI => {
            let v = match bv {
                BoundValue::Literal(LiteralValue::Integer(n)) => *n as f64,
                BoundValue::Literal(LiteralValue::Decimal(f)) => *f,
                BoundValue::Literal(LiteralValue::Float(f)) => *f as f64,
                BoundValue::Literal(LiteralValue::Double(f)) => *f,
                BoundValue::Literal(LiteralValue::Boolean(b)) => {
                    if *b {
                        1.0
                    } else {
                        0.0
                    }
                }
                BoundValue::Literal(LiteralValue::String(s)) if valid_double_lex(s.trim()) => {
                    s.trim().parse::<f64>().ok()?
                }
                BoundValue::Literal(LiteralValue::Typed { value, datatype })
                    if matches!(
                        datatype.as_str(),
                        XSD_INTEGER_IRI | XSD_DECIMAL_IRI | XSD_FLOAT_IRI | XSD_DOUBLE_IRI
                    ) =>
                {
                    let num = Numeric::from_literal(&LiteralValue::Typed {
                        value: value.clone(),
                        datatype: datatype.clone(),
                    })?;
                    num.as_f64()
                }
                _ => return None,
            };
            Some(BoundValue::Literal(LiteralValue::Double(v)))
        }
        XSD_FLOAT_IRI => {
            let v = match bv {
                BoundValue::Literal(LiteralValue::Integer(n)) => *n as f32,
                BoundValue::Literal(LiteralValue::Decimal(f)) => *f as f32,
                BoundValue::Literal(LiteralValue::Float(f)) => *f,
                BoundValue::Literal(LiteralValue::Double(f)) => *f as f32,
                BoundValue::Literal(LiteralValue::Boolean(b)) => {
                    if *b {
                        1.0
                    } else {
                        0.0
                    }
                }
                BoundValue::Literal(LiteralValue::String(s)) if valid_double_lex(s.trim()) => {
                    s.trim().parse::<f32>().ok()?
                }
                BoundValue::Literal(LiteralValue::Typed { value, datatype })
                    if matches!(
                        datatype.as_str(),
                        XSD_INTEGER_IRI | XSD_DECIMAL_IRI | XSD_FLOAT_IRI | XSD_DOUBLE_IRI
                    ) =>
                {
                    let num = Numeric::from_literal(&LiteralValue::Typed {
                        value: value.clone(),
                        datatype: datatype.clone(),
                    })?;
                    num.as_f64() as f32
                }
                _ => return None,
            };
            Some(BoundValue::Literal(LiteralValue::Float(v)))
        }
        _ => None,
    }
}

fn valid_integer_lex(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && matches!(b[i], b'+' | b'-') {
        i += 1;
    }
    if i >= b.len() {
        return false;
    }
    b[i..].iter().all(|c| c.is_ascii_digit())
}

fn valid_decimal_lex(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && matches!(b[i], b'+' | b'-') {
        i += 1;
    }
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    digits > 0 && i == b.len()
}

fn valid_double_lex(s: &str) -> bool {
    if matches!(s, "INF" | "+INF" | "-INF" | "NaN") {
        return true;
    }
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && matches!(b[i], b'+' | b'-') {
        i += 1;
    }
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    let mut saw_dot = false;
    if i < b.len() && b[i] == b'.' {
        saw_dot = true;
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    if i < b.len() && matches!(b[i], b'e' | b'E') {
        i += 1;
        if i < b.len() && matches!(b[i], b'+' | b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return false;
        }
    }
    i == b.len() && (digits > 0 || saw_dot)
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
        Algebra::Path {
            subject, object, ..
        } => {
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

/// Strips SELECT `Project` layers (through Slice/Distinct/OrderBy) so that
/// projection expressions can reference variables trimmed by the projection.
fn strip_select_projection(algebra: &Algebra) -> Algebra {
    match algebra {
        Algebra::Project { input, .. } => strip_select_projection(input),
        Algebra::Slice {
            offset,
            limit,
            input,
        } => Algebra::Slice {
            offset: *offset,
            limit: *limit,
            input: Box::new(strip_select_projection(input)),
        },
        Algebra::Distinct { input } => Algebra::Distinct {
            input: Box::new(strip_select_projection(input)),
        },
        Algebra::OrderBy { keys, input } => Algebra::OrderBy {
            keys: keys.clone(),
            input: Box::new(strip_select_projection(input)),
        },
        other => other.clone(),
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

fn materialize_construct(
    template: &[TriplePattern],
    solutions: &[Solution],
    ctx: &ExecCtx<'_>,
) -> Vec<Triple> {
    let mut out = Vec::new();
    for sol in solutions {
        for pattern in template {
            if let (Some(s), Some(p), Some(o)) = (
                instantiate_node(&pattern.subject, sol, ctx),
                instantiate_iri(&pattern.predicate, sol),
                instantiate_term(&pattern.object, sol),
            ) {
                out.push(Triple::new(s, p, o));
            }
        }
    }
    out
}

fn instantiate_node(p: &TermPattern, sol: &Solution, ctx: &ExecCtx<'_>) -> Option<NodeId> {
    match p {
        TermPattern::Node(n) => Some(*n),
        TermPattern::Iri(i) => ctx.read.node_for_iri(i).ok().flatten(),
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

/// `EXISTS { pattern }` evaluated with the current solution bindings:
/// the solution becomes a single-row VALUES joined with the pattern.
fn exists_in_solution(pattern: &Algebra, sol: &Solution, ctx: &ExecCtx<'_>) -> Option<bool> {
    let variables: Vec<String> = sol.bindings.keys().cloned().collect();
    let bindings = vec![
        sol.bindings
            .values()
            .map(|v| Some(term_pattern_from_bound(v)))
            .collect(),
    ];
    let combined = Algebra::Join {
        left: Box::new(Algebra::Values {
            variables,
            bindings,
        }),
        right: Box::new(pattern.clone()),
    };
    eval_algebra(&combined, ctx)
        .map(|rows| !rows.is_empty())
        .ok()
}

fn term_pattern_from_bound(bv: &BoundValue) -> TermPattern {
    match bv {
        BoundValue::Node(n) => TermPattern::Node(*n),
        BoundValue::Iri(i) => TermPattern::Iri(i.clone()),
        BoundValue::Literal(l) => TermPattern::Literal(l.clone()),
        BoundValue::Blank(n) => TermPattern::Node(*n),
    }
}
