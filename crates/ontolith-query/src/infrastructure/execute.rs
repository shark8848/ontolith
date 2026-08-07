//! Algebra executor producing solution bindings (L3).

use crate::application::{QueryReadService, UpdateWriteService};
use crate::domain::{
    AggregateFunction, AggregateSpec, Algebra, BoundValue, Expression, GraphTarget, PathExpression,
    PreemptionReason, PreemptionToken, QueryKind, QueryPlan, QueryRequest, QueryResult, Solution,
    TermPattern, TriplePattern, UpdateOp,
};
use ontolith_core::domain::{Iri, LiteralValue, NodeId};
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
                let triples = materialize_construct(&plan.construct_template, &solutions);
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
                            if let Some(v) = eval_expr_value(&pe.expression, s) {
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
        Algebra::Filter { expression, input } => {
            let rows = eval_algebra(input, ctx)?;
            let mut out = Vec::new();
            for s in rows {
                ctx.check()?;
                if eval_expr_bool(expression, &s).unwrap_or(false) {
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
        out_rows.retain(|row| eval_expr_bool(having, row) == Some(true));
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
        AggregateFunction::Sum { variable } => {
            let mut acc_i: i64 = 0;
            let mut acc_d: f64 = 0.0;
            let mut all_int = true;
            let mut any = false;
            for s in rows {
                let Some(bv) = s.get(variable) else { continue };
                if let BoundValue::Literal(LiteralValue::Integer(x)) = bv {
                    acc_i += x;
                    acc_d += *x as f64;
                    any = true;
                } else if let Some(x) = numeric_value(bv) {
                    acc_d += x;
                    all_int = false;
                    any = true;
                }
            }
            if !any {
                None
            } else if all_int {
                Some(BoundValue::Literal(LiteralValue::Integer(acc_i)))
            } else {
                Some(BoundValue::Literal(LiteralValue::Decimal(acc_d)))
            }
        }
        AggregateFunction::Avg { variable } => {
            let mut acc_d: f64 = 0.0;
            let mut n: i64 = 0;
            for s in rows {
                if let Some(x) = s.get(variable).and_then(numeric_value) {
                    acc_d += x;
                    n += 1;
                }
            }
            if n == 0 {
                None
            } else {
                Some(BoundValue::Literal(LiteralValue::Decimal(acc_d / n as f64)))
            }
        }
        AggregateFunction::Min { variable } => extremum(variable, rows, -1),
        AggregateFunction::Max { variable } => extremum(variable, rows, 1),
    }
}

fn numeric_value(bv: &BoundValue) -> Option<f64> {
    match bv {
        BoundValue::Literal(LiteralValue::Integer(x)) => Some(*x as f64),
        BoundValue::Literal(LiteralValue::Decimal(x)) => Some(*x),
        _ => None,
    }
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
        (
            BoundValue::Node(a) | BoundValue::Blank(a),
            BoundValue::Node(b) | BoundValue::Blank(b),
        ) => Ok(a == b),
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
        Expression::Negate(e) => match eval_expr_value(e, sol)? {
            BoundValue::Literal(LiteralValue::Integer(n)) => Some(n != 0),
            BoundValue::Literal(LiteralValue::Decimal(f)) => Some(f != 0.0),
            _ => None,
        },
        Expression::Arith { op, left, right } => {
            let l = eval_expr_value(left, sol)?;
            let r = eval_expr_value(right, sol)?;
            let ln = bound_as_f64(&l)?;
            let rn = bound_as_f64(&r)?;
            let v = match op {
                '+' => ln + rn,
                '-' => ln - rn,
                '*' => ln * rn,
                '/' => {
                    if rn == 0.0 {
                        return None;
                    }
                    ln / rn
                }
                _ => return None,
            };
            Some(v != 0.0)
        }
        Expression::Function { .. } => match eval_expr_value(expr, sol)? {
            BoundValue::Literal(LiteralValue::Boolean(b)) => Some(b),
            _ => None,
        },
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
        Expression::Negate(e) => match eval_expr_value(e, sol)? {
            BoundValue::Literal(LiteralValue::Integer(n)) => {
                Some(BoundValue::Literal(LiteralValue::Integer(-n)))
            }
            BoundValue::Literal(LiteralValue::Decimal(f)) => {
                Some(BoundValue::Literal(LiteralValue::Decimal(-f)))
            }
            _ => None,
        },
        Expression::Arith { op, left, right } => {
            let l = eval_expr_value(left, sol)?;
            let r = eval_expr_value(right, sol)?;
            let ln = bound_as_f64(&l)?;
            let rn = bound_as_f64(&r)?;
            let v = match op {
                '+' => ln + rn,
                '-' => ln - rn,
                '*' => ln * rn,
                '/' => {
                    if rn == 0.0 {
                        return None;
                    }
                    ln / rn
                }
                _ => return None,
            };
            Some(match (l, r) {
                (
                    BoundValue::Literal(LiteralValue::Integer(_)),
                    BoundValue::Literal(LiteralValue::Integer(_)),
                ) if *op != '/' => BoundValue::Literal(LiteralValue::Integer(v as i64)),
                _ => BoundValue::Literal(LiteralValue::Decimal(v)),
            })
        }
        Expression::Function { name, args } => eval_function(name, args, sol),
        // Aggregates are only valid in HAVING and are rewritten to their
        // projection alias before execution; a stray call is an error.
        Expression::Aggregate(_) => None,
    }
}

/// Built-in SPARQL function subset over the compact literal model (P3-05).
fn eval_function(name: &str, args: &[Expression], sol: &Solution) -> Option<BoundValue> {
    let arg = |i: usize| eval_expr_value(args.get(i)?, sol);
    let string_of = |bv: &BoundValue| -> Option<String> {
        match bv {
            BoundValue::Literal(l) => Some(l.lexical_form()),
            BoundValue::Iri(i) => Some(i.as_str().to_owned()),
            _ => None,
        }
    };
    let string = |i: usize| string_of(&arg(i)?);
    let int_of = |bv: &BoundValue| -> Option<i64> {
        match bv {
            BoundValue::Literal(LiteralValue::Integer(n)) => Some(*n),
            BoundValue::Literal(LiteralValue::Decimal(f)) if f.fract() == 0.0 => Some(*f as i64),
            _ => None,
        }
    };
    let num_of = |bv: &BoundValue| -> Option<f64> {
        match bv {
            BoundValue::Literal(LiteralValue::Integer(n)) => Some(*n as f64),
            BoundValue::Literal(LiteralValue::Decimal(f)) => Some(*f),
            _ => None,
        }
    };
    let bool = |b: bool| BoundValue::Literal(LiteralValue::Boolean(b));
    let strlit = |s: String| BoundValue::Literal(LiteralValue::String(s));

    match name {
        "STR" => match arg(0)? {
            BoundValue::Iri(i) => Some(strlit(i.as_str().to_owned())),
            BoundValue::Literal(l) => Some(strlit(l.lexical_form())),
            _ => None,
        },
        "UCASE" => Some(strlit(string(0)?.to_uppercase())),
        "LCASE" => Some(strlit(string(0)?.to_lowercase())),
        "STRLEN" => Some(BoundValue::Literal(LiteralValue::Integer(
            string(0)?.chars().count() as i64,
        ))),
        "CONCAT" => {
            let mut out = String::new();
            for i in 0..args.len() {
                out.push_str(&string(i)?);
            }
            Some(strlit(out))
        }
        "SUBSTR" => {
            let s = string(0)?;
            let start = int_of(&arg(1)?)?;
            let chars: Vec<char> = s.chars().collect();
            let from = (start.max(1) as usize - 1).min(chars.len());
            let out: String = if args.len() >= 3 {
                let len = int_of(&arg(2)?)?.max(0) as usize;
                chars.iter().skip(from).take(len).collect()
            } else {
                chars.iter().skip(from).collect()
            };
            Some(strlit(out))
        }
        "CONTAINS" => Some(bool(string(0)?.contains(&string(1)?))),
        "STRSTARTS" => Some(bool(string(0)?.starts_with(&string(1)?))),
        "STRENDS" => Some(bool(string(0)?.ends_with(&string(1)?))),
        "STRAFTER" => {
            let s = string(0)?;
            let needle = string(1)?;
            match s.find(&needle) {
                Some(idx) => Some(strlit(s[idx + needle.len()..].to_owned())),
                None => Some(strlit(String::new())),
            }
        }
        "STRBEFORE" => {
            let s = string(0)?;
            let needle = string(1)?;
            match s.find(&needle) {
                Some(idx) => Some(strlit(s[..idx].to_owned())),
                None => Some(strlit(String::new())),
            }
        }
        "LANG" => Some(strlit(String::new())),
        "DATATYPE" => match arg(0)? {
            BoundValue::Literal(l) => Some(BoundValue::Iri(l.xsd_datatype_iri())),
            _ => None,
        },
        "IRI" | "URI" => Some(BoundValue::Iri(Iri::new(string(0)?))),
        "ABS" => match num_of(&arg(0)?) {
            Some(f) if f >= 0.0 => Some(BoundValue::Literal(LiteralValue::Decimal(f))),
            Some(f) => Some(BoundValue::Literal(LiteralValue::Decimal(-f))),
            None => None,
        },
        "CEIL" => Some(BoundValue::Literal(LiteralValue::Integer(
            num_of(&arg(0)?)?.ceil() as i64,
        ))),
        "FLOOR" => Some(BoundValue::Literal(LiteralValue::Integer(
            num_of(&arg(0)?)?.floor() as i64,
        ))),
        "ROUND" => Some(BoundValue::Literal(LiteralValue::Integer(
            num_of(&arg(0)?)?.round() as i64,
        ))),
        "ISNUMERIC" => Some(bool(matches!(
            arg(0)?,
            BoundValue::Literal(LiteralValue::Integer(_) | LiteralValue::Decimal(_))
        ))),
        "IF" => {
            let cond = eval_expr_bool(&args[0], sol)?;
            if cond {
                eval_expr_value(args.get(1)?, sol)
            } else {
                eval_expr_value(args.get(2)?, sol)
            }
        }
        "COALESCE" => {
            for a in args {
                if let Some(v) = eval_expr_value(a, sol) {
                    return Some(v);
                }
            }
            None
        }
        "IN" | "NOT IN" => {
            let needle = eval_expr_value(args.first()?, sol)?;
            let mut found = false;
            for a in args.iter().skip(1) {
                if let Some(v) = eval_expr_value(a, sol)
                    && v == needle
                {
                    found = true;
                    break;
                }
            }
            Some(bool(if name == "IN" { found } else { !found }))
        }
        _ => None,
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

/// Numeric value of a bound literal for arithmetic (SPARQL numeric promotion).
fn bound_as_f64(bv: &BoundValue) -> Option<f64> {
    match bv {
        BoundValue::Literal(LiteralValue::Integer(n)) => Some(*n as f64),
        BoundValue::Literal(LiteralValue::Decimal(f)) => Some(*f),
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
