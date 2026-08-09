//! SPARQL 1.1 Query parser (core surface for L3).
//!
//! Produces a [`QueryPlan`] with algebra covering:
//! SELECT / ASK / CONSTRUCT, WHERE groups, OPTIONAL, UNION, FILTER, BIND,
//! VALUES, DISTINCT, ORDER BY, LIMIT/OFFSET, PREFIX/BASE.

use crate::domain::{
    AggregateExpr, AggregateFunction, AggregateSpec, Algebra, Expression, GraphRef, GraphTarget,
    OrderKey, PathExpression, ProjectionExpr, QueryKind, QueryPlan, QueryPlanId, QueryRequest,
    TermPattern, TriplePattern, UpdateOp, UpdatePattern,
};
use ontolith_core::domain::{Iri, LanguageTag, LiteralValue};
use ontolith_core::error::OntolithError;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

pub fn plan_query(request: &QueryRequest) -> Result<QueryPlan, OntolithError> {
    let text = request.query.0.as_str();
    if text.trim().is_empty() {
        return Err(OntolithError::InvalidArgument("query text is empty"));
    }
    let mut parser = SparqlParser::new(text);
    parser.parse_query()
}

struct SparqlParser<'a> {
    input: &'a str,
    pos: usize,
    line: usize,
    col: usize,
    prefixes: BTreeMap<String, String>,
    base: Option<String>,
    logical: Vec<String>,
    blank_counter: u64,
    /// Expansion triples/patterns produced by blank node property lists and
    /// RDF collections, pending collection into the enclosing BGP/template.
    pending_group: Vec<Algebra>,
    /// Aliases bound by the enclosing SELECT expression/aggregate projection.
    /// A subquery must not project a variable with one of these names
    /// (SPARQL 1.1 §18.2.2 variable scope).
    outer_select_expr_vars: Vec<String>,
}

impl<'a> SparqlParser<'a> {
    fn new(input: &'a str) -> Self {
        let mut prefixes = BTreeMap::new();
        prefixes.insert(
            "rdf".into(),
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#".into(),
        );
        prefixes.insert(
            "rdfs".into(),
            "http://www.w3.org/2000/01/rdf-schema#".into(),
        );
        prefixes.insert("xsd".into(), "http://www.w3.org/2001/XMLSchema#".into());
        Self {
            input,
            pos: 0,
            line: 1,
            col: 1,
            prefixes,
            base: None,
            logical: vec!["normalize_query".into()],
            blank_counter: 0,
            pending_group: Vec::new(),
            outer_select_expr_vars: Vec::new(),
        }
    }

    fn parse_query(&mut self) -> Result<QueryPlan, OntolithError> {
        self.skip();
        while self.looking_at_keyword("PREFIX") || self.looking_at_keyword("BASE") {
            if self.looking_at_keyword("PREFIX") {
                self.parse_prefix()?;
            } else {
                self.parse_base()?;
            }
            self.skip();
        }

        let kind = if self.eat_keyword("SELECT") {
            QueryKind::Select
        } else if self.eat_keyword("ASK") {
            QueryKind::Ask
        } else if self.eat_keyword("CONSTRUCT") {
            QueryKind::Construct
        } else if self.eat_keyword("DESCRIBE") {
            QueryKind::Describe
        } else if self.looking_at_keyword("INSERT")
            || self.looking_at_keyword("DELETE")
            || self.looking_at_keyword("WITH")
            || self.looking_at_keyword("LOAD")
            || self.looking_at_keyword("CLEAR")
            || self.looking_at_keyword("DROP")
            || self.looking_at_keyword("ADD")
            || self.looking_at_keyword("COPY")
            || self.looking_at_keyword("MOVE")
            || self.looking_at_keyword("CREATE")
        {
            QueryKind::Update
        } else {
            // Legacy bare patterns / subject= hints still accepted as SELECT.
            QueryKind::Select
        };
        self.logical.push(format!("detect_kind:{}", kind.as_str()));

        if kind == QueryKind::Describe {
            return Ok(QueryPlan {
                id: plan_id(self.input),
                kind,
                algebra: Algebra::Identity,
                update_ops: Vec::new(),
                prefixes: self.prefixes.clone(),
                base: self.base.clone(),
                from: Vec::new(),
                from_named: Vec::new(),
                logical_steps: self.logical.clone(),
                physical_steps: vec![format!("unsupported:{}", kind.as_str())],
                construct_template: Vec::new(),
                estimated_rows: None,
                pattern_costs: Vec::new(),
                projection_exprs: Vec::new(),
            });
        }

        if kind == QueryKind::Update {
            let update_ops = self.parse_update_ops()?;
            return Ok(QueryPlan {
                id: plan_id(self.input),
                kind,
                algebra: Algebra::Identity,
                update_ops,
                prefixes: self.prefixes.clone(),
                base: self.base.clone(),
                from: Vec::new(),
                from_named: Vec::new(),
                logical_steps: self.logical.clone(),
                physical_steps: vec!["update:ops".to_string()],
                construct_template: Vec::new(),
                estimated_rows: None,
                pattern_costs: Vec::new(),
                projection_exprs: Vec::new(),
            });
        }

        let mut distinct = false;
        let mut select_vars: Vec<String> = Vec::new();
        let mut plain_vars: Vec<String> = Vec::new();
        let mut aggregates: Vec<AggregateSpec> = Vec::new();
        let mut projection_exprs: Vec<ProjectionExpr> = Vec::new();
        let mut star_projection = false;
        let mut construct_template = Vec::new();
        let mut construct_where_consumed = false;

        if kind == QueryKind::Select {
            self.skip();
            if self.eat_keyword("DISTINCT") {
                distinct = true;
                self.logical.push("distinct".into());
            }
            self.skip();
            if self.peek_char() == Some('*') {
                self.bump();
                star_projection = true;
                self.logical.push("project:*".into());
            } else {
                loop {
                    self.skip();
                    if self.peek_char() == Some('?') || self.peek_char() == Some('$') {
                        let v = self.parse_var_name()?;
                        if select_vars.contains(&v) {
                            return Err(self.err(format!("duplicate projected variable ?{v}")));
                        }
                        select_vars.push(v.clone());
                        plain_vars.push(v);
                    } else if self.peek_char() == Some('(') {
                        // Peek past '(' to distinguish aggregate specs from
                        // generic projection expressions `(expr AS ?alias)`.
                        let save = self.checkpoint();
                        self.bump(); // '('
                        self.skip();
                        let is_aggregate = self.looking_at_keyword("COUNT")
                            || self.looking_at_keyword("SUM")
                            || self.looking_at_keyword("AVG")
                            || self.looking_at_keyword("MIN")
                            || self.looking_at_keyword("MAX")
                            || self.looking_at_keyword("GROUP_CONCAT")
                            || self.looking_at_keyword("SAMPLE");
                        self.restore(save);
                        if is_aggregate {
                            let spec = self.parse_aggregate_spec()?;
                            if select_vars.contains(&spec.output) {
                                return Err(self.err(format!(
                                    "duplicate projected variable ?{}",
                                    spec.output
                                )));
                            }
                            select_vars.push(spec.output.clone());
                            aggregates.push(spec);
                        } else {
                            self.bump(); // '('
                            self.skip();
                            let expression =
                                lift_aggregates(self.parse_expression()?, &mut aggregates)?;
                            self.skip();
                            self.expect_keyword("AS")?;
                            self.skip();
                            let alias = self.parse_var_name()?;
                            self.skip();
                            self.expect_char(')')?;
                            if select_vars.contains(&alias) {
                                return Err(
                                    self.err(format!("duplicate projected variable ?{alias}"))
                                );
                            }
                            select_vars.push(alias.clone());
                            projection_exprs.push(ProjectionExpr { expression, alias });
                        }
                    } else {
                        break;
                    }
                }
                if select_vars.is_empty()
                    && aggregates.is_empty()
                    && projection_exprs.is_empty()
                    && self.looking_at_keyword("WHERE")
                {
                    // SELECT WHERE without vars → *
                    star_projection = true;
                    self.logical.push("project:*".into());
                } else if !select_vars.is_empty() {
                    self.logical
                        .push(format!("project:{}", select_vars.join(",")));
                }
                if !aggregates.is_empty() {
                    self.logical
                        .push(format!("aggregates:{}", aggregates.len()));
                }
                if !projection_exprs.is_empty() {
                    self.logical
                        .push(format!("project_exprs:{}", projection_exprs.len()));
                }
            }
        } else if kind == QueryKind::Construct {
            self.skip();
            if self.looking_at_keyword("WHERE") {
                // CONSTRUCT WHERE { pattern } — the template is the pattern.
                self.eat_keyword("WHERE");
                self.skip();
                if self.peek_char() == Some('{') {
                    let tpl = self.parse_construct_template()?;
                    construct_template = tpl.clone();
                    self.logical.push(format!("construct_where:{}", tpl.len()));
                    construct_where_consumed = true;
                }
            } else if self.peek_char() == Some('{') {
                construct_template = self.parse_construct_template()?;
                self.logical
                    .push(format!("construct_template:{}", construct_template.len()));
            }
        }

        self.skip();
        let (from, from_named) = self.parse_dataset_clauses()?;
        if !from.is_empty() {
            self.logical.push(format!("from:{}", from.len()));
        }
        if !from_named.is_empty() {
            self.logical
                .push(format!("from_named:{}", from_named.len()));
        }
        // `CONSTRUCT FROM <g> WHERE { pattern }` shorthand: the WHERE pattern
        // doubles as the template when no explicit template was given.
        if kind == QueryKind::Construct
            && construct_template.is_empty()
            && !construct_where_consumed
            && self.looking_at_keyword("WHERE")
        {
            self.eat_keyword("WHERE");
            self.skip();
            if self.peek_char() == Some('{') {
                let tpl = self.parse_construct_template()?;
                construct_template = tpl.clone();
                self.logical.push(format!("construct_where:{}", tpl.len()));
                construct_where_consumed = true;
            }
        }
        // WHERE is optional for ASK { } form; CONSTRUCT WHERE already consumed it.
        if !construct_where_consumed {
            let _ = self.eat_keyword("WHERE");
        }
        self.skip();

        // Projected expression/aggregate aliases are in scope for the WHERE
        // clause; a subquery must not project a variable with one of these
        // names (SPARQL 1.1 §18.2.2).
        self.outer_select_expr_vars = projection_exprs
            .iter()
            .map(|e| e.alias.clone())
            .chain(aggregates.iter().map(|a| a.output.clone()))
            .collect();

        let mut body = if construct_where_consumed {
            Algebra::Bgp(construct_template.clone())
        } else if self.peek_char() == Some('{') {
            self.parse_group_graph_pattern()?
        } else if let Some(hint_subj) = parse_subject_hint(self.input)? {
            // legacy full-scan with subject hint
            self.logical.push("apply_subject_filter".into());
            Algebra::Bgp(vec![TriplePattern {
                subject: TermPattern::Node(hint_subj),
                predicate: TermPattern::Variable("p".into()),
                object: TermPattern::Variable("o".into()),
            }])
        } else {
            Algebra::Bgp(vec![TriplePattern {
                subject: TermPattern::Variable("s".into()),
                predicate: TermPattern::Variable("p".into()),
                object: TermPattern::Variable("o".into()),
            }])
        };
        // Legacy `# subject=N` specializes unbound subjects even when WHERE is present.
        if let Some(hint_subj) = parse_subject_hint(self.input)?
            && apply_subject_hint(&mut body, hint_subj)
        {
            self.logical.push("apply_subject_filter".into());
        }
        self.logical.push(format!("where:{}", algebra_tag(&body)));

        if star_projection && !aggregates.is_empty() {
            return Err(OntolithError::query(
                "SELECT * cannot be mixed with aggregate expressions",
            ));
        }

        self.skip();

        // GROUP BY — appears after the WHERE group, before solution modifiers.
        let mut groups: Vec<String> = Vec::new();
        if self.eat_keyword("GROUP") {
            if star_projection {
                return Err(self.err("SELECT * cannot be combined with GROUP BY"));
            }
            self.skip();
            self.expect_keyword("BY")?;
            loop {
                self.skip();
                if self.peek_char() == Some('?') || self.peek_char() == Some('$') {
                    groups.push(self.parse_var_name()?);
                } else if self.peek_char() == Some('(') {
                    self.bump();
                    self.skip();
                    let expr = self.parse_expression()?;
                    self.skip();
                    self.expect_keyword("AS")?;
                    self.skip();
                    let alias = self.parse_var_name()?;
                    self.skip();
                    self.expect_char(')')?;
                    groups.push(alias.clone());
                    body = Algebra::Extend {
                        variable: alias,
                        expression: expr,
                        input: Box::new(body),
                    };
                } else {
                    break;
                }
            }
            self.logical.push(format!("group_by:{}", groups.len()));
            self.skip();
        }

        // HAVING — one or more constraints applied to the grouped result
        // (may reference aggregate aliases).
        let mut having: Option<Expression> = None;
        while self.eat_keyword("HAVING") {
            self.skip();
            while self.peek_char() == Some('(') {
                let constraint = lift_aggregates(self.parse_constraint()?, &mut aggregates)?;
                having = Some(match having {
                    None => constraint,
                    Some(prev) => Expression::And(Box::new(prev), Box::new(constraint)),
                });
                self.logical.push("having".into());
                self.skip();
            }
        }

        if !aggregates.is_empty() && !plain_vars.is_empty() && groups.is_empty() {
            return Err(OntolithError::query(
                "mixed aggregate and non-aggregate projection requires GROUP BY",
            ));
        }
        if !groups.is_empty() {
            for v in &plain_vars {
                if !groups.contains(v) {
                    return Err(OntolithError::query(format!(
                        "projected variable ?{v} must appear in GROUP BY"
                    )));
                }
            }
        }

        // solution modifiers
        let mut algebra = body;

        if !aggregates.is_empty() || !groups.is_empty() {
            algebra = Algebra::Aggregate {
                groups,
                aggregates,
                having,
                input: Box::new(algebra),
            };
        }

        self.skip();

        // ORDER BY
        if self.eat_keyword("ORDER") {
            self.skip();
            self.expect_keyword("BY")?;
            let mut keys = Vec::new();
            self.skip();
            loop {
                let ascending = if self.eat_keyword("DESC") {
                    self.skip();
                    false
                } else {
                    let _ = self.eat_keyword("ASC");
                    self.skip();
                    true
                };
                if self.peek_char() == Some('(') {
                    self.bump();
                    self.skip();
                }
                if self.peek_char() == Some('?') || self.peek_char() == Some('$') {
                    let v = self.parse_var_name()?;
                    keys.push(OrderKey {
                        variable: v,
                        ascending,
                    });
                } else {
                    break;
                }
                self.skip();
                if self.peek_char() == Some(')') {
                    self.bump();
                    self.skip();
                }
                if !(self.peek_char() == Some('?')
                    || self.peek_char() == Some('$')
                    || self.looking_at_keyword("ASC")
                    || self.looking_at_keyword("DESC"))
                {
                    break;
                }
            }
            if !keys.is_empty() {
                self.logical.push(format!("order_by:{}", keys.len()));
                algebra = Algebra::OrderBy {
                    keys,
                    input: Box::new(algebra),
                };
            }
        }

        let mut offset = 0usize;
        let mut limit = None;
        self.skip();
        if self.eat_keyword("OFFSET") {
            self.skip();
            offset = self.parse_usize()?;
            self.logical.push(format!("offset:{offset}"));
            self.skip();
        }
        if self.eat_keyword("LIMIT") {
            self.skip();
            limit = Some(self.parse_usize()?);
            self.logical.push(format!("limit:{}", limit.unwrap()));
            self.skip();
        }
        // LIMIT may appear before OFFSET
        if limit.is_none() && self.eat_keyword("LIMIT") {
            self.skip();
            limit = Some(self.parse_usize()?);
            self.logical.push(format!("limit:{}", limit.unwrap()));
        }
        if offset == 0 && self.eat_keyword("OFFSET") {
            self.skip();
            offset = self.parse_usize()?;
            self.logical.push(format!("offset:{offset}"));
        }

        if distinct {
            algebra = Algebra::Distinct {
                input: Box::new(algebra),
            };
        }

        if kind == QueryKind::Select && !star_projection {
            algebra = Algebra::Project {
                variables: select_vars,
                input: Box::new(algebra),
            };
        }

        if offset > 0 || limit.is_some() {
            algebra = Algebra::Slice {
                offset,
                limit,
                input: Box::new(algebra),
            };
        }

        // SPARQL 1.1 trailing VALUES clause (`SELECT ... WHERE {...} VALUES ...`)
        // joins the whole query result with the values table.
        if kind == QueryKind::Select || kind == QueryKind::Ask || kind == QueryKind::Construct {
            self.skip();
            if self.eat_keyword("VALUES") {
                let values = self.parse_values()?;
                algebra = join(algebra, values);
                self.logical.push("post_values".into());
            }
        }

        self.skip();
        if !self.eof() {
            return Err(self.err("unexpected trailing content after query"));
        }

        let physical = physical_steps(&algebra);
        Ok(QueryPlan {
            id: plan_id(self.input),
            kind,
            algebra,
            update_ops: Vec::new(),
            prefixes: self.prefixes.clone(),
            base: self.base.clone(),
            from,
            from_named,
            logical_steps: self.logical.clone(),
            physical_steps: physical,
            construct_template,
            estimated_rows: None,
            pattern_costs: Vec::new(),
            projection_exprs,
        })
    }

    fn parse_prefix(&mut self) -> Result<(), OntolithError> {
        self.expect_keyword("PREFIX")?;
        self.skip();
        let name = self.parse_prefixed_name_left()?;
        self.skip();
        let iri = self.parse_iriref()?;
        self.prefixes.insert(name, iri);
        self.logical.push("prefix".into());
        Ok(())
    }

    fn parse_base(&mut self) -> Result<(), OntolithError> {
        self.expect_keyword("BASE")?;
        self.skip();
        let iri = self.parse_iriref()?;
        self.base = Some(iri);
        self.logical.push("base".into());
        Ok(())
    }

    fn parse_construct_template(&mut self) -> Result<Vec<TriplePattern>, OntolithError> {
        self.expect_char('{')?;
        let mut patterns = Vec::new();
        self.skip();
        while self.peek_char() != Some('}') && !self.eof() {
            if let Some(p) = self.try_parse_triple_pattern()? {
                patterns.extend(self.parse_triple_semicolon_chain(p)?);
                patterns.extend(self.drain_template_patterns()?);
            } else {
                break;
            }
            self.skip();
            if self.peek_char() == Some('.') {
                self.bump();
                self.skip();
            }
        }
        self.expect_char('}')?;
        Ok(patterns)
    }

    /// Update template / DATA block / DELETE WHERE body: `{ ... }` with triple
    /// patterns, `;`/`,` shorthands and `GRAPH <g> { ... }` blocks. Returns
    /// patterns with their target named graph (`None` = operation default).
    fn parse_update_block(
        &mut self,
        allow_bnodes: bool,
    ) -> Result<Vec<UpdatePattern>, OntolithError> {
        self.expect_char('{')?;
        let mut patterns = Vec::new();
        self.skip();
        while self.peek_char() != Some('}') && !self.eof() {
            self.skip();
            if self.looking_at_keyword("GRAPH") {
                let save = self.checkpoint();
                self.eat_keyword("GRAPH");
                self.skip();
                if self.peek_char() == Some('<') || self.looking_at_prefixed_name() {
                    let g = Iri::new(self.parse_iri_or_prefixed()?);
                    self.skip();
                    let inner = self.parse_update_block(allow_bnodes)?;
                    for p in inner {
                        if p.graph.is_some() {
                            return Err(self.err("nested GRAPH blocks are not allowed"));
                        }
                        patterns.push(UpdatePattern {
                            graph: Some(g.clone()),
                            triple: p.triple,
                        });
                    }
                    self.skip();
                    if self.peek_char() == Some('.') {
                        self.bump();
                        self.skip();
                    }
                    continue;
                }
                self.restore(save);
            }
            if let Some(p) = self.try_parse_triple_pattern()? {
                for triple in self.parse_triple_semicolon_chain(p)? {
                    if !allow_bnodes
                        && (matches!(triple.subject, TermPattern::Blank(_))
                            || matches!(triple.object, TermPattern::Blank(_)))
                    {
                        return Err(self.err("blank nodes are not allowed in DELETE templates"));
                    }
                    patterns.push(UpdatePattern {
                        graph: None,
                        triple,
                    });
                }
                for triple in self.drain_template_patterns()? {
                    patterns.push(UpdatePattern {
                        graph: None,
                        triple,
                    });
                }
            } else {
                break;
            }
            self.skip();
            if self.peek_char() == Some('.') {
                self.bump();
                self.skip();
            }
        }
        self.expect_char('}')?;
        Ok(patterns)
    }

    fn parse_data_block(
        &mut self,
        allow_bnodes: bool,
    ) -> Result<Vec<UpdatePattern>, OntolithError> {
        let patterns = self.parse_update_block(true)?;
        let is_real_var = |tp: &TermPattern| matches!(tp, TermPattern::Variable(_));
        for p in &patterns {
            let t = &p.triple;
            // Blank labels are legal in INSERT/DELETE DATA; only true
            // variables are rejected there.
            if is_real_var(&t.subject)
                || is_real_var(&t.predicate)
                || is_real_var(&t.object)
                || (!allow_bnodes
                    && (matches!(t.subject, TermPattern::Blank(_))
                        || matches!(t.object, TermPattern::Blank(_))))
            {
                return Err(self.err("DATA block triples must be concrete (no variables)"));
            }
        }
        Ok(patterns)
    }

    /// Parse an IRI that may be either an `<IRIREF>` or a prefixed name.
    fn parse_iri_or_prefixed(&mut self) -> Result<String, OntolithError> {
        self.skip();
        if self.peek_char() == Some('<') {
            self.parse_iriref()
        } else {
            let word = self.parse_word()?;
            if word.is_empty() {
                return Err(self.err("expected IRI"));
            }
            self.expand_prefixed(&word)
        }
    }

    fn looking_at_prefixed_name(&self) -> bool {
        // Accept `ex:local` and the default prefix `:local`; a bare `:` or
        // `ex:` followed by a delimiter is rejected (invalid anyway).
        let rest = &self.input[self.pos..];
        for (i, c) in rest.chars().enumerate() {
            if c == ':' {
                return rest[i + 1..].chars().next().is_some_and(|nc| {
                    nc.is_ascii_alphanumeric() || nc == '_' || nc == '-' || nc == ':'
                });
            }
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                continue;
            }
            return false;
        }
        false
    }

    fn parse_update_ops(&mut self) -> Result<Vec<UpdateOp>, OntolithError> {
        // SPARQL Update request: operations separated by `;` (a single
        // trailing `;` is tolerated). Blank labels in INSERT/DELETE DATA are
        // request-scoped and must not be reused across operations; labels in
        // modify templates are operation-scoped variables and may repeat.
        let mut ops = Vec::new();
        let mut seen_data_blanks: BTreeSet<String> = BTreeSet::new();
        self.skip();
        loop {
            let op = self.parse_single_update_op()?;
            // Deduplicate within the operation: reusing a label inside one
            // DATA block denotes the same blank node and is legal.
            let op_blanks: BTreeSet<String> = data_op_blank_labels(&op).into_iter().collect();
            for label in op_blanks {
                if !seen_data_blanks.insert(label.clone()) {
                    return Err(self.err(format!(
                        "blank node label '_:{label}' reused across DATA operations"
                    )));
                }
            }
            ops.push(op);
            self.skip();
            let has_separator = self.eat_operator(";");
            if has_separator {
                self.skip();
                if !self.eof() && !self.looking_at_update_keyword() {
                    return Err(self.err("expected update operation after ';'"));
                }
                if !self.eof() {
                    continue;
                }
            }
            break;
        }
        self.skip();
        if !self.eof() {
            return Err(self.err("unexpected trailing content after update request"));
        }
        self.logical.push(format!("update_ops:{}", ops.len()));
        Ok(ops)
    }

    fn parse_single_update_op(&mut self) -> Result<UpdateOp, OntolithError> {
        self.skip();
        let op;
        if self.looking_at_keyword("INSERT") {
            self.eat_keyword("INSERT");
            self.skip();
            if self.eat_keyword("DATA") {
                self.skip();
                op = UpdateOp::InsertData(self.parse_data_block(true)?);
            } else {
                let insert = self.parse_update_block(true)?;
                self.skip();
                let (using, using_named) = self.parse_using_clause()?;
                self.expect_keyword("WHERE")?;
                self.skip();
                let where_pattern = self.parse_group_graph_pattern()?;
                op = UpdateOp::DeleteInsert {
                    graph: None,
                    using,
                    using_named,
                    delete: Vec::new(),
                    insert,
                    where_pattern,
                };
            }
        } else if self.looking_at_keyword("DELETE") {
            self.eat_keyword("DELETE");
            self.skip();
            if self.eat_keyword("DATA") {
                self.skip();
                op = UpdateOp::DeleteData(self.parse_data_block(false)?);
            } else if self.looking_at_keyword("WHERE") {
                self.eat_keyword("WHERE");
                self.skip();
                op = UpdateOp::DeleteWhere {
                    graph: None,
                    using: Vec::new(),
                    using_named: Vec::new(),
                    patterns: self.parse_update_block(false)?,
                };
            } else {
                let delete = self.parse_update_block(false)?;
                self.skip();
                let mut insert = Vec::new();
                if self.eat_keyword("INSERT") {
                    self.skip();
                    insert = self.parse_update_block(true)?;
                    self.skip();
                }
                let (using, using_named) = self.parse_using_clause()?;
                self.expect_keyword("WHERE")?;
                self.skip();
                let where_pattern = self.parse_group_graph_pattern()?;
                op = UpdateOp::DeleteInsert {
                    graph: None,
                    using,
                    using_named,
                    delete,
                    insert,
                    where_pattern,
                };
            }
        } else if self.eat_keyword("WITH") {
            self.skip();
            let graph = Iri::new(self.parse_iri_or_prefixed()?);
            self.skip();
            if self.looking_at_keyword("DELETE") {
                self.eat_keyword("DELETE");
                self.skip();
                if self.looking_at_keyword("WHERE") {
                    self.eat_keyword("WHERE");
                    self.skip();
                    op = UpdateOp::DeleteWhere {
                        graph: Some(graph),
                        using: Vec::new(),
                        using_named: Vec::new(),
                        patterns: self.parse_update_block(false)?,
                    };
                } else {
                    let delete = self.parse_update_block(false)?;
                    self.skip();
                    let mut insert = Vec::new();
                    if self.eat_keyword("INSERT") {
                        self.skip();
                        insert = self.parse_update_block(true)?;
                        self.skip();
                    }
                    let (using, using_named) = self.parse_using_clause()?;
                    self.expect_keyword("WHERE")?;
                    self.skip();
                    let where_pattern = self.parse_group_graph_pattern()?;
                    op = UpdateOp::DeleteInsert {
                        graph: Some(graph),
                        using,
                        using_named,
                        delete,
                        insert,
                        where_pattern,
                    };
                }
            } else if self.looking_at_keyword("INSERT") {
                self.eat_keyword("INSERT");
                self.skip();
                let insert = self.parse_update_block(true)?;
                self.skip();
                let (using, using_named) = self.parse_using_clause()?;
                self.expect_keyword("WHERE")?;
                self.skip();
                let where_pattern = self.parse_group_graph_pattern()?;
                op = UpdateOp::DeleteInsert {
                    graph: Some(graph),
                    using,
                    using_named,
                    delete: Vec::new(),
                    insert,
                    where_pattern,
                };
            } else {
                return Err(self.err("expected DELETE or INSERT after WITH"));
            }
        } else if self.looking_at_keyword("CLEAR") || self.looking_at_keyword("DROP") {
            let is_drop = self.eat_keyword("DROP");
            if !is_drop {
                self.eat_keyword("CLEAR");
            }
            let silent = self.eat_keyword("SILENT");
            self.skip();
            let target = if self.eat_keyword("DEFAULT") {
                GraphTarget::Default
            } else if self.eat_keyword("NAMED") {
                GraphTarget::Named
            } else if self.eat_keyword("ALL") {
                GraphTarget::All
            } else if self.eat_keyword("GRAPH") {
                self.skip();
                GraphTarget::Graph(Iri::new(self.parse_iri_or_prefixed()?))
            } else {
                return Err(self.err("expected DEFAULT/NAMED/ALL/GRAPH after CLEAR/DROP"));
            };
            op = if is_drop {
                UpdateOp::Drop { silent, target }
            } else {
                UpdateOp::Clear { silent, target }
            };
        } else if self.looking_at_keyword("LOAD") {
            self.eat_keyword("LOAD");
            let silent = self.eat_keyword("SILENT");
            self.skip();
            let source = Iri::new(self.parse_iri_or_prefixed()?);
            self.skip();
            let into = if self.eat_keyword("INTO") {
                self.skip();
                self.expect_keyword("GRAPH")?;
                self.skip();
                Some(Iri::new(self.parse_iri_or_prefixed()?))
            } else {
                None
            };
            op = UpdateOp::Load {
                silent,
                source,
                into,
            };
        } else if self.looking_at_keyword("ADD")
            || self.looking_at_keyword("COPY")
            || self.looking_at_keyword("MOVE")
        {
            let op_code = if self.eat_keyword("ADD") {
                'A'
            } else if self.eat_keyword("COPY") {
                'C'
            } else {
                self.eat_keyword("MOVE");
                'M'
            };
            let silent = self.eat_keyword("SILENT");
            self.skip();
            let from = self.parse_graph_ref()?;
            self.skip();
            self.expect_keyword("TO")?;
            self.skip();
            let to = self.parse_graph_ref()?;
            op = match op_code {
                'A' => UpdateOp::Add { silent, from, to },
                'C' => UpdateOp::Copy { silent, from, to },
                _ => UpdateOp::Move { silent, from, to },
            };
        } else if self.looking_at_keyword("CREATE") {
            self.eat_keyword("CREATE");
            let silent = self.eat_keyword("SILENT");
            self.skip();
            self.expect_keyword("GRAPH")?;
            self.skip();
            let graph = Iri::new(self.parse_iri_or_prefixed()?);
            op = UpdateOp::Create { silent, graph };
        } else {
            return Err(self.err(
                "expected update operation INSERT/DELETE/CLEAR/DROP/LOAD/ADD/COPY/MOVE/CREATE",
            ));
        }
        Ok(op)
    }

    fn looking_at_update_keyword(&self) -> bool {
        self.looking_at_keyword("INSERT")
            || self.looking_at_keyword("DELETE")
            || self.looking_at_keyword("WITH")
            || self.looking_at_keyword("LOAD")
            || self.looking_at_keyword("CLEAR")
            || self.looking_at_keyword("DROP")
            || self.looking_at_keyword("ADD")
            || self.looking_at_keyword("COPY")
            || self.looking_at_keyword("MOVE")
            || self.looking_at_keyword("CREATE")
    }

    /// `USING <g>` / `USING NAMED <g>`* clause of a modify form.
    fn parse_using_clause(&mut self) -> Result<(Vec<Iri>, Vec<Iri>), OntolithError> {
        let mut using = Vec::new();
        let mut using_named = Vec::new();
        self.skip();
        while self.eat_keyword("USING") {
            self.skip();
            if self.eat_keyword("NAMED") {
                self.skip();
                using_named.push(Iri::new(self.parse_iri_or_prefixed()?));
            } else {
                using.push(Iri::new(self.parse_iri_or_prefixed()?));
            }
            self.skip();
        }
        Ok((using, using_named))
    }

    fn parse_graph_ref(&mut self) -> Result<GraphRef, OntolithError> {
        self.skip();
        if self.eat_keyword("DEFAULT") {
            return Ok(GraphRef::Default);
        }
        // GraphOrDefault: `DEFAULT` | `GRAPH`? iri — GRAPH is optional.
        self.eat_keyword("GRAPH");
        self.skip();
        Ok(GraphRef::Graph(Iri::new(self.parse_iri_or_prefixed()?)))
    }

    /// `DatasetClause*` from the query prologue: `FROM <iri>` fills the
    /// default graph and `FROM NAMED <iri>` the named graphs of the query
    /// dataset (SPARQL 1.1 §18.2.1).
    fn parse_dataset_clauses(&mut self) -> Result<(Vec<Iri>, Vec<Iri>), OntolithError> {
        let mut from = Vec::new();
        let mut from_named = Vec::new();
        loop {
            self.skip();
            if !self.looking_at_keyword("FROM") {
                break;
            }
            self.eat_keyword("FROM");
            self.skip();
            if self.eat_keyword("NAMED") {
                self.skip();
                from_named.push(Iri::new(self.parse_iri_or_prefixed()?));
            } else {
                from.push(Iri::new(self.parse_iri_or_prefixed()?));
            }
        }
        Ok((from, from_named))
    }

    fn parse_aggregate_spec(&mut self) -> Result<AggregateSpec, OntolithError> {
        self.expect_char('(')?;
        self.skip();
        let function = self.parse_aggregate_call()?;
        self.skip();
        self.expect_keyword("AS")?;
        self.skip();
        let output = self.parse_var_name()?;
        self.skip();
        self.expect_char(')')?;

        Ok(AggregateSpec { function, output })
    }

    fn parse_aggregate_call(&mut self) -> Result<AggregateFunction, OntolithError> {
        if self.looking_at_keyword("COUNT") {
            self.eat_keyword("COUNT");
            self.skip();
            self.expect_char('(')?;
            self.skip();
            let distinct = self.eat_keyword("DISTINCT");
            self.skip();
            let expr = if self.peek_char() == Some('*') {
                self.bump();
                None
            } else {
                Some(self.parse_aggregate_expr()?)
            };
            self.skip();
            self.expect_char(')')?;
            Ok(AggregateFunction::Count { expr, distinct })
        } else if self.looking_at_keyword("SUM") {
            self.eat_keyword("SUM");
            let (expr, distinct) = self.parse_aggregate_arg()?;
            Ok(AggregateFunction::Sum { expr, distinct })
        } else if self.looking_at_keyword("AVG") {
            self.eat_keyword("AVG");
            let (expr, distinct) = self.parse_aggregate_arg()?;
            Ok(AggregateFunction::Avg { expr, distinct })
        } else if self.looking_at_keyword("MIN") {
            self.eat_keyword("MIN");
            let (expr, distinct) = self.parse_aggregate_arg()?;
            Ok(AggregateFunction::Min { expr, distinct })
        } else if self.looking_at_keyword("MAX") {
            self.eat_keyword("MAX");
            let (expr, distinct) = self.parse_aggregate_arg()?;
            Ok(AggregateFunction::Max { expr, distinct })
        } else if self.looking_at_keyword("GROUP_CONCAT") {
            self.eat_keyword("GROUP_CONCAT");
            self.skip();
            self.expect_char('(')?;
            self.skip();
            let distinct = self.eat_keyword("DISTINCT");
            self.skip();
            let expr = self.parse_aggregate_expr()?;
            self.skip();
            let separator = if self.eat_operator(";") {
                self.skip();
                self.expect_keyword("SEPARATOR")?;
                self.skip();
                self.expect_char('=')?;
                self.skip();
                let sep = self.parse_string_literal()?;
                self.skip();
                self.expect_char(')')?;
                sep.lexical_form()
            } else {
                self.expect_char(')')?;
                " ".to_owned()
            };
            Ok(AggregateFunction::GroupConcat {
                expr,
                distinct,
                separator,
            })
        } else if self.looking_at_keyword("SAMPLE") {
            self.eat_keyword("SAMPLE");
            let (expr, _) = self.parse_aggregate_arg()?;
            Ok(AggregateFunction::Sample { expr })
        } else {
            Err(self.err("expected aggregate function COUNT/SUM/AVG/MIN/MAX"))
        }
    }

    fn parse_aggregate_arg(&mut self) -> Result<(AggregateExpr, bool), OntolithError> {
        self.skip();
        self.expect_char('(')?;
        self.skip();
        let distinct = self.eat_keyword("DISTINCT");
        self.skip();
        let expr = self.parse_aggregate_expr()?;
        self.skip();
        self.expect_char(')')?;
        Ok((expr, distinct))
    }

    /// Aggregate argument: a bare variable (kept as fast-path lookup) or an
    /// arbitrary expression (`AVG(IF(...))`, `GROUP_CONCAT(?x)`).
    fn parse_aggregate_expr(&mut self) -> Result<AggregateExpr, OntolithError> {
        let expr = self.parse_expression()?;
        Ok(match expr {
            Expression::Variable(v) => AggregateExpr::Variable(v),
            other => AggregateExpr::Expression(Box::new(other)),
        })
    }

    fn parse_group_graph_pattern(&mut self) -> Result<Algebra, OntolithError> {
        self.expect_char('{')?;
        let alg = self.parse_group_graph_pattern_sub()?;
        self.skip();
        self.expect_char('}')?;
        Ok(alg)
    }

    fn parse_group_graph_pattern_sub(&mut self) -> Result<Algebra, OntolithError> {
        let mut acc = Algebra::Identity;
        // SPARQL 1.1 §18.2.2.6 translates a group's FILTERs as wrapping the
        // whole group (after its BIND extensions), so a FILTER can see a
        // variable bound by a BIND later in the same group (w3c bind08).
        let mut pending_filters: Vec<Expression> = Vec::new();
        self.skip();
        let group_start = self.pos;
        while !self.eof() && self.peek_char() != Some('}') {
            if self.eat_keyword("OPTIONAL") {
                self.skip();
                let right = self.parse_group_graph_pattern()?;
                acc = Algebra::LeftJoin {
                    left: Box::new(acc),
                    right: Box::new(right),
                    condition: None,
                };
                self.logical.push("optional".into());
            } else if self.eat_keyword("UNION") {
                // UNION binds tighter with previous unit — handled as binary
                self.skip();
                let right = if self.peek_char() == Some('{') {
                    self.parse_group_graph_pattern()?
                } else {
                    return Err(self.err("UNION requires a group"));
                };
                acc = Algebra::Union {
                    left: Box::new(acc),
                    right: Box::new(right),
                };
                self.logical.push("union".into());
            } else if self.eat_keyword("MINUS") {
                self.skip();
                let right = self.parse_group_graph_pattern()?;
                acc = Algebra::Minus {
                    left: Box::new(acc),
                    right: Box::new(right),
                };
                self.logical.push("minus".into());
            } else if self.eat_keyword("FILTER") {
                self.skip();
                let expr = self.parse_constraint()?;
                pending_filters.push(expr);
                self.logical.push("filter".into());
            } else if self.eat_keyword("BIND") {
                self.skip();
                self.expect_char('(')?;
                self.skip();
                let expr = self.parse_expression()?;
                self.skip();
                self.expect_keyword("AS")?;
                self.skip();
                let var = self.parse_var_name()?;
                self.skip();
                self.expect_char(')')?;
                if bindings_in_algebra(&acc).contains(&var) {
                    return Err(self.err(format!(
                        "BIND variable ?{var} is already bound in this group"
                    )));
                }
                acc = Algebra::Extend {
                    variable: var,
                    expression: expr,
                    input: Box::new(acc),
                };
                self.logical.push("bind".into());
            } else if self.eat_keyword("VALUES") {
                let values = self.parse_values()?;
                acc = join(acc, values);
                self.logical.push("values".into());
            } else if self.eat_keyword("GRAPH") {
                self.skip();
                let graph = self.parse_var_or_term(false)?;
                self.skip();
                let inner = self.parse_group_graph_pattern()?;
                acc = join(
                    acc,
                    Algebra::Graph {
                        graph,
                        inner: Box::new(inner),
                    },
                );
                self.logical.push("graph".into());
            } else if self.looking_at_keyword("SELECT") {
                // A `SubSelect` is only legal as the sole content of a group
                // (`{ SELECT ... }`); a bare SELECT after other elements is a
                // syntax error (w3c syn-bad-07).
                if self.pos != group_start {
                    return Err(
                        self.err("subquery is only allowed as the first element of a group")
                    );
                }
                let subquery = self.parse_subquery_select()?;
                acc = join(acc, subquery);
                self.logical.push("subquery".into());
            } else if self.peek_char() == Some('{') {
                // Nested group or Union left side already in group: `{ A } UNION { B }`
                let nested = self.parse_group_graph_pattern()?;
                self.skip();
                if self.eat_keyword("UNION") {
                    self.skip();
                    let right = self.parse_group_graph_pattern()?;
                    let u = Algebra::Union {
                        left: Box::new(nested),
                        right: Box::new(right),
                    };
                    acc = join(acc, u);
                    self.logical.push("union".into());
                } else {
                    acc = join(acc, nested);
                }
            } else if let Some(path) = self.try_parse_property_path()? {
                self.skip();
                if self.peek_char() == Some('.') {
                    self.bump();
                }
                self.logical.push("property_path".into());
                acc = join(acc, path);
            } else if let Some(pattern) = self.try_parse_triple_pattern()? {
                // collect consecutive triple patterns into one BGP
                let mut bgp = self.parse_triple_semicolon_chain(pattern)?;
                self.skip();
                while self.peek_char() == Some('.') {
                    self.bump();
                    self.skip();
                    if self.peek_char() == Some('}')
                        || self.looking_at_keyword("OPTIONAL")
                        || self.looking_at_keyword("FILTER")
                        || self.looking_at_keyword("BIND")
                        || self.looking_at_keyword("VALUES")
                        || self.looking_at_keyword("UNION")
                        || self.peek_char() == Some('{')
                    {
                        break;
                    }
                    if let Some(p) = self.try_parse_triple_pattern()? {
                        bgp.extend(self.parse_triple_semicolon_chain(p)?);
                        self.skip();
                    } else {
                        break;
                    }
                }
                // trailing dot
                if self.peek_char() == Some('.') {
                    self.bump();
                }
                self.logical.push(format!("bgp:{}", bgp.len()));
                acc = join(acc, Algebra::Bgp(bgp));
                for item in self.drain_pending() {
                    acc = join(acc, item);
                }
            } else if self.peek_char() == Some('[') {
                // Blank node property list as a standalone pattern
                // (`[ :p ?o ]`); expansion is queued by the parser. Runs only
                // after triple/path forms fail, so `[ :p ?o ] ...` (list as a
                // triple subject) is still parsed as a triple.
                let _term = self.parse_bnode_property_list()?;
                for item in self.drain_pending() {
                    acc = join(acc, item);
                }
                self.logical.push("bnode_property_list".into());
                self.skip();
                if self.peek_char() == Some('.') {
                    self.bump();
                }
            } else {
                break;
            }
            self.skip();
        }
        for expr in pending_filters {
            acc = Algebra::Filter {
                expression: expr,
                input: Box::new(acc),
            };
        }
        Ok(acc)
    }

    fn try_parse_property_path(&mut self) -> Result<Option<Algebra>, OntolithError> {
        self.skip();
        let save = self.checkpoint();

        let subject = match self.parse_var_or_term(true) {
            Ok(v) => v,
            Err(_) => {
                self.restore(save);
                return Ok(None);
            }
        };

        self.skip();
        let pred_start = self.checkpoint();
        let is_path = if matches!(self.peek_char(), Some('^') | Some('!') | Some('(')) {
            true
        } else {
            match self.parse_var_or_term(false) {
                Ok(TermPattern::Iri(_)) => {
                    // Path modifiers (`?`/`*`/`+`) bind tightly to the IRI
                    // with no whitespace; otherwise `?o` is a variable object.
                    let adjacent_modifier =
                        matches!(self.peek_char(), Some('?') | Some('*') | Some('+'));
                    self.skip();
                    matches!(self.peek_char(), Some('/') | Some('|')) || adjacent_modifier
                }
                _ => false,
            }
        };
        self.restore(pred_start);
        if !is_path {
            self.restore(save);
            return Ok(None);
        }

        let path = match self.parse_path_alternative() {
            Ok(path) => path,
            Err(_) => {
                self.restore(save);
                return Ok(None);
            }
        };

        self.skip();
        let object = match self.parse_var_or_term(false) {
            Ok(v) => v,
            Err(_) => {
                self.restore(save);
                return Ok(None);
            }
        };

        Ok(Some(Algebra::Path {
            subject,
            path,
            object,
        }))
    }

    fn parse_path_alternative(&mut self) -> Result<PathExpression, OntolithError> {
        let mut left = self.parse_path_sequence()?;
        self.skip();
        while self.peek_char() == Some('|') {
            self.bump();
            self.skip();
            let right = self.parse_path_sequence()?;
            left = PathExpression::Alternative(Box::new(left), Box::new(right));
            self.skip();
        }
        Ok(left)
    }

    fn parse_path_sequence(&mut self) -> Result<PathExpression, OntolithError> {
        let mut left = self.parse_path_unary()?;
        self.skip();
        while self.peek_char() == Some('/') {
            self.bump();
            self.skip();
            let right = self.parse_path_unary()?;
            left = PathExpression::Sequence(Box::new(left), Box::new(right));
            self.skip();
        }
        Ok(left)
    }

    fn parse_path_unary(&mut self) -> Result<PathExpression, OntolithError> {
        self.skip();
        let mut base = if self.peek_char() == Some('!') {
            self.bump();
            self.parse_negated_property_set()?
        } else if self.peek_char() == Some('(') {
            self.bump();
            self.skip();
            let inner = self.parse_path_alternative()?;
            self.skip();
            self.expect_char(')')?;
            inner
        } else if self.peek_char() == Some('^') {
            self.bump();
            self.skip();
            if self.peek_char() == Some('(') {
                self.bump();
                self.skip();
                let inner = self.parse_path_alternative()?;
                self.skip();
                self.expect_char(')')?;
                invert_path(inner)
            } else {
                match self.parse_var_or_term(false)? {
                    TermPattern::Iri(iri) => PathExpression::InversePredicate(iri),
                    _ => return Err(self.err("inverse property path requires IRI predicate")),
                }
            }
        } else {
            match self.parse_var_or_term(false)? {
                TermPattern::Iri(iri) => PathExpression::Predicate(iri),
                _ => return Err(self.err("property path requires IRI predicates")),
            }
        };

        // Path modifiers bind directly to the path element: a whitespace
        // separated `?x` is a variable object, not a `?` modifier.
        if self.peek_char() == Some('+') {
            self.bump();
            base = PathExpression::OneOrMore(Box::new(base));
        } else if self.peek_char() == Some('*') {
            self.bump();
            base = PathExpression::ZeroOrMore(Box::new(base));
        } else if self.peek_char() == Some('?') {
            self.bump();
            base = PathExpression::ZeroOrOne(Box::new(base));
        }

        Ok(base)
    }

    /// Negated property set after `!`: `a`, `^a`, `p`, `^p`, or
    /// `(p1 | ^p2 | a | ...)` (each element forward or inverse).
    fn parse_negated_property_set(&mut self) -> Result<PathExpression, OntolithError> {
        self.skip();
        let mut forward = Vec::new();
        let mut reverse = Vec::new();
        let in_parens = self.peek_char() == Some('(');
        if in_parens {
            self.bump();
            self.skip();
        }
        loop {
            self.skip();
            let mut inverse = false;
            if self.peek_char() == Some('^') {
                self.bump();
                self.skip();
                inverse = true;
            }
            let iri = if self.looking_at_keyword("a") {
                self.eat_bare_name();
                Iri::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type")
            } else if self.peek_char() == Some('<') {
                let raw = self.parse_iriref()?;
                if raw.is_empty() || raw.chars().any(|c| c.is_ascii_whitespace()) {
                    return Err(self.err("invalid IRI reference"));
                }
                Iri::new(raw)
            } else {
                let word = self.parse_word()?;
                if word.is_empty() {
                    return Err(self.err("negated property set requires an IRI"));
                }
                Iri::parse(self.expand_prefixed(&word)?).map_err(|e| self.err(e.message()))?
            };
            if inverse {
                reverse.push(iri);
            } else {
                forward.push(iri);
            }
            if in_parens {
                self.skip();
                if self.peek_char() == Some('|') {
                    self.bump();
                    continue;
                }
                self.expect_char(')')?;
                break;
            }
            break;
        }
        Ok(PathExpression::NegatedPropertySet { forward, reverse })
    }

    fn parse_subquery_select(&mut self) -> Result<Algebra, OntolithError> {
        self.expect_keyword("SELECT")?;
        self.skip();

        let saved_outer_vars = std::mem::take(&mut self.outer_select_expr_vars);
        let result = self.parse_subquery_select_inner(saved_outer_vars.clone());
        self.outer_select_expr_vars = saved_outer_vars;
        result
    }

    fn parse_subquery_select_inner(
        &mut self,
        outer_expr_vars: Vec<String>,
    ) -> Result<Algebra, OntolithError> {
        self.skip();

        let mut distinct = false;
        let mut select_vars: Vec<String> = Vec::new();
        let mut plain_vars: Vec<String> = Vec::new();
        let mut aggregates: Vec<AggregateSpec> = Vec::new();
        let mut projection_exprs: Vec<ProjectionExpr> = Vec::new();

        if self.eat_keyword("DISTINCT") {
            distinct = true;
            self.skip();
        }

        if self.peek_char() == Some('*') {
            self.bump();
            self.skip();
        } else {
            loop {
                self.skip();
                if self.peek_char() == Some('?') || self.peek_char() == Some('$') {
                    let v = self.parse_var_name()?;
                    select_vars.push(v.clone());
                    plain_vars.push(v);
                } else if self.peek_char() == Some('(') {
                    // Distinguish aggregate specs from generic projection
                    // expressions `(expr AS ?alias)`.
                    let save = self.checkpoint();
                    self.bump(); // '('
                    self.skip();
                    let is_aggregate = self.looking_at_keyword("COUNT")
                        || self.looking_at_keyword("SUM")
                        || self.looking_at_keyword("AVG")
                        || self.looking_at_keyword("MIN")
                        || self.looking_at_keyword("MAX")
                        || self.looking_at_keyword("GROUP_CONCAT")
                        || self.looking_at_keyword("SAMPLE");
                    self.restore(save);
                    if is_aggregate {
                        let spec = self.parse_aggregate_spec()?;
                        select_vars.push(spec.output.clone());
                        aggregates.push(spec);
                    } else {
                        self.bump(); // '('
                        self.skip();
                        let expression =
                            lift_aggregates(self.parse_expression()?, &mut aggregates)?;
                        self.skip();
                        self.expect_keyword("AS")?;
                        self.skip();
                        let alias = self.parse_var_name()?;
                        self.skip();
                        self.expect_char(')')?;
                        select_vars.push(alias.clone());
                        projection_exprs.push(ProjectionExpr { expression, alias });
                    }
                } else {
                    break;
                }
            }
            if select_vars.is_empty() && aggregates.is_empty() && projection_exprs.is_empty() {
                return Err(self.err("subquery SELECT requires '*' or variables"));
            }
        }

        for v in &select_vars {
            if outer_expr_vars.iter().any(|outer| outer == v) {
                return Err(self.err(format!(
                    "subquery projects ?{v}, which is bound by the enclosing SELECT expression"
                )));
            }
        }
        self.outer_select_expr_vars = projection_exprs
            .iter()
            .map(|e| e.alias.clone())
            .chain(aggregates.iter().map(|a| a.output.clone()))
            .collect();

        let _ = self.eat_keyword("WHERE");
        self.skip();
        if self.peek_char() != Some('{') {
            return Err(self.err("subquery SELECT requires group graph pattern"));
        }

        let mut algebra = self.parse_group_graph_pattern()?;

        self.skip();

        let mut groups: Vec<String> = Vec::new();
        if self.eat_keyword("GROUP") {
            self.skip();
            self.expect_keyword("BY")?;
            loop {
                self.skip();
                if self.peek_char() == Some('?') || self.peek_char() == Some('$') {
                    groups.push(self.parse_var_name()?);
                } else if self.peek_char() == Some('(') {
                    self.bump();
                    self.skip();
                    let expr = self.parse_expression()?;
                    self.skip();
                    self.expect_keyword("AS")?;
                    self.skip();
                    let alias = self.parse_var_name()?;
                    self.skip();
                    self.expect_char(')')?;
                    groups.push(alias.clone());
                    algebra = Algebra::Extend {
                        variable: alias,
                        expression: expr,
                        input: Box::new(algebra),
                    };
                } else {
                    break;
                }
            }
            self.skip();
        }

        let mut having: Option<Expression> = None;
        if self.looking_at_keyword("HAVING") {
            self.eat_keyword("HAVING");
            self.skip();
            while self.peek_char() == Some('(') {
                let constraint = lift_aggregates(self.parse_constraint()?, &mut aggregates)?;
                having = Some(match having {
                    None => constraint,
                    Some(prev) => Expression::And(Box::new(prev), Box::new(constraint)),
                });
                self.skip();
            }
        }

        if !aggregates.is_empty() && !plain_vars.is_empty() && groups.is_empty() {
            return Err(self.err("mixed aggregate and non-aggregate projection requires GROUP BY"));
        }
        if !groups.is_empty() {
            for v in &plain_vars {
                if !groups.contains(v) {
                    return Err(
                        self.err(format!("projected variable ?{v} must appear in GROUP BY"))
                    );
                }
            }
        }

        if !aggregates.is_empty() || !groups.is_empty() {
            algebra = Algebra::Aggregate {
                groups,
                aggregates,
                having,
                input: Box::new(algebra),
            };
        }
        for expr in &projection_exprs {
            algebra = Algebra::Extend {
                variable: expr.alias.clone(),
                expression: expr.expression.clone(),
                input: Box::new(algebra),
            };
        }

        self.skip();
        if self.eat_keyword("ORDER") {
            self.skip();
            self.expect_keyword("BY")?;
            let mut keys = Vec::new();
            self.skip();
            loop {
                let ascending = if self.eat_keyword("DESC") {
                    self.skip();
                    false
                } else {
                    let _ = self.eat_keyword("ASC");
                    self.skip();
                    true
                };
                if self.peek_char() == Some('(') {
                    self.bump();
                    self.skip();
                }
                if self.peek_char() == Some('?') || self.peek_char() == Some('$') {
                    let v = self.parse_var_name()?;
                    keys.push(OrderKey {
                        variable: v,
                        ascending,
                    });
                } else {
                    break;
                }
                self.skip();
                if self.peek_char() == Some(')') {
                    self.bump();
                    self.skip();
                }
                if !(self.peek_char() == Some('?')
                    || self.peek_char() == Some('$')
                    || self.looking_at_keyword("ASC")
                    || self.looking_at_keyword("DESC"))
                {
                    break;
                }
            }
            if !keys.is_empty() {
                algebra = Algebra::OrderBy {
                    keys,
                    input: Box::new(algebra),
                };
            }
        }

        let mut offset = 0usize;
        let mut limit = None;
        self.skip();
        if self.eat_keyword("OFFSET") {
            self.skip();
            offset = self.parse_usize()?;
            self.skip();
        }
        if self.eat_keyword("LIMIT") {
            self.skip();
            limit = Some(self.parse_usize()?);
            self.skip();
        }
        if limit.is_none() && self.eat_keyword("LIMIT") {
            self.skip();
            limit = Some(self.parse_usize()?);
            self.skip();
        }
        if offset == 0 && self.eat_keyword("OFFSET") {
            self.skip();
            offset = self.parse_usize()?;
            self.skip();
        }

        if distinct {
            algebra = Algebra::Distinct {
                input: Box::new(algebra),
            };
        }

        algebra = Algebra::Project {
            variables: select_vars,
            input: Box::new(algebra),
        };

        if offset > 0 || limit.is_some() {
            algebra = Algebra::Slice {
                offset,
                limit,
                input: Box::new(algebra),
            };
        }

        Ok(algebra)
    }

    fn parse_values(&mut self) -> Result<Algebra, OntolithError> {
        self.skip();
        let mut variables = Vec::new();
        if self.peek_char() == Some('?') || self.peek_char() == Some('$') {
            variables.push(self.parse_var_name()?);
        } else {
            self.expect_char('(')?;
            self.skip();
            while self.peek_char() != Some(')') && !self.eof() {
                variables.push(self.parse_var_name()?);
                self.skip();
            }
            self.expect_char(')')?;
        }
        self.skip();
        self.expect_char('{')?;
        let mut bindings = Vec::new();
        self.skip();
        while self.peek_char() != Some('}') && !self.eof() {
            if variables.len() == 1 && self.peek_char() != Some('(') {
                let term = self.parse_graph_term_or_undef()?;
                bindings.push(vec![term]);
            } else {
                self.expect_char('(')?;
                self.skip();
                let mut row = Vec::new();
                for _ in 0..variables.len() {
                    row.push(self.parse_graph_term_or_undef()?);
                    self.skip();
                }
                self.expect_char(')')?;
                bindings.push(row);
            }
            self.skip();
        }
        self.expect_char('}')?;
        Ok(Algebra::Values {
            variables,
            bindings,
        })
    }

    fn parse_graph_term_or_undef(&mut self) -> Result<Option<TermPattern>, OntolithError> {
        self.skip();
        if self.eat_keyword("UNDEF") {
            return Ok(None);
        }
        Ok(Some(self.parse_graph_term()?))
    }

    fn try_parse_triple_pattern(&mut self) -> Result<Option<TriplePattern>, OntolithError> {
        self.skip();
        let save = self.checkpoint();
        match self.parse_triple_pattern_inner() {
            Ok(p) => Ok(Some(p)),
            Err(_) => {
                self.restore(save);
                Ok(None)
            }
        }
    }

    /// Semicolon shorthand: `?s :p1 ?o1; :p2 ?o2` reuses the subject across
    /// consecutive predicate-object pairs.
    fn parse_triple_semicolon_chain(
        &mut self,
        mut pattern: TriplePattern,
    ) -> Result<Vec<TriplePattern>, OntolithError> {
        let mut out = vec![pattern.clone()];
        self.skip();
        loop {
            if self.eat_operator(";") {
                self.skip();
                let subject = pattern.subject.clone();
                let predicate = if self.eat_keyword("a") {
                    TermPattern::Iri(Iri::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"))
                } else {
                    self.parse_var_or_term(false)?
                };
                let object = self.parse_var_or_term(false)?;
                pattern = TriplePattern {
                    subject: subject.clone(),
                    predicate: predicate.clone(),
                    object,
                };
                out.push(pattern.clone());
                self.skip();
            } else if self.eat_operator(",") {
                self.skip();
                let object = self.parse_var_or_term(false)?;
                pattern = TriplePattern {
                    subject: pattern.subject.clone(),
                    predicate: pattern.predicate.clone(),
                    object,
                };
                out.push(pattern.clone());
                self.skip();
            } else {
                break;
            }
        }
        Ok(out)
    }

    fn parse_triple_pattern_inner(&mut self) -> Result<TriplePattern, OntolithError> {
        let subject = self.parse_var_or_term(true)?;
        self.skip();
        let predicate = if self.eat_keyword("a") {
            TermPattern::Iri(Iri::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"))
        } else {
            self.parse_var_or_term(false)?
        };
        self.skip();
        let object = self.parse_var_or_term(false)?;
        Ok(TriplePattern {
            subject,
            predicate,
            object,
        })
    }

    fn fresh_blank(&mut self) -> String {
        self.blank_counter += 1;
        format!("_gen_{}", self.blank_counter)
    }

    fn drain_pending(&mut self) -> Vec<Algebra> {
        std::mem::take(&mut self.pending_group)
    }

    /// Drain pending expansions from templates: only plain triples are legal
    /// in CONSTRUCT/update templates (property paths are a query-side form).
    fn drain_template_patterns(&mut self) -> Result<Vec<TriplePattern>, OntolithError> {
        let mut out = Vec::new();
        for item in self.drain_pending() {
            match item {
                Algebra::Bgp(patterns) => out.extend(patterns),
                _ => return Err(self.err("property paths are not allowed in templates")),
            }
        }
        Ok(out)
    }

    /// `[ p o ; p2 o2 ]` — anonymous blank node with a property list. Property
    /// predicates may be paths (`[ :p|:q ?x ]`). Returns the blank term and
    /// queues the expansion (triples or path patterns) into `pending_group`.
    fn parse_bnode_property_list(&mut self) -> Result<TermPattern, OntolithError> {
        self.expect_char('[')?;
        self.skip();
        let label = self.fresh_blank();
        if self.peek_char() == Some(']') {
            self.bump();
            return Ok(TermPattern::Blank(label));
        }
        loop {
            self.skip();
            let (verb, path) = self.parse_verb_or_path()?;
            self.skip();
            let object = self.parse_var_or_term(false)?;
            let subject = TermPattern::Blank(label.clone());
            if let Some(path) = path {
                self.pending_group.push(Algebra::Path {
                    subject,
                    path,
                    object,
                });
            } else {
                self.pending_group.push(Algebra::Bgp(vec![TriplePattern {
                    subject,
                    predicate: verb,
                    object,
                }]));
            }
            self.skip();
            if self.eat_operator(";") {
                continue;
            }
            break;
        }
        self.skip();
        self.expect_char(']')?;
        Ok(TermPattern::Blank(label))
    }

    /// `( item ... )` — RDF collection; items may be terms, blank node
    /// property lists or nested collections. Returns the head term and queues
    /// the rdf:first/rdf:rest expansion into `pending_group`.
    fn parse_collection(&mut self) -> Result<TermPattern, OntolithError> {
        self.expect_char('(')?;
        self.skip();
        let mut items = Vec::new();
        while self.peek_char() != Some(')') && !self.eof() {
            items.push(self.parse_var_or_term(false)?);
            self.skip();
        }
        self.expect_char(')')?;
        if items.is_empty() {
            return Ok(TermPattern::Iri(Iri::new(
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil",
            )));
        }
        let rdf_first =
            TermPattern::Iri(Iri::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#first"));
        let rdf_rest =
            TermPattern::Iri(Iri::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#rest"));
        let rdf_nil = TermPattern::Iri(Iri::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#nil"));
        let head = TermPattern::Blank(self.fresh_blank());
        let mut prev = head.clone();
        for (i, item) in items.iter().enumerate() {
            self.pending_group.push(Algebra::Bgp(vec![TriplePattern {
                subject: prev.clone(),
                predicate: rdf_first.clone(),
                object: item.clone(),
            }]));
            let rest_object = if i + 1 < items.len() {
                let next = TermPattern::Blank(self.fresh_blank());
                self.pending_group.push(Algebra::Bgp(vec![TriplePattern {
                    subject: prev.clone(),
                    predicate: rdf_rest.clone(),
                    object: next.clone(),
                }]));
                prev = next;
                continue;
            } else {
                rdf_nil.clone()
            };
            self.pending_group.push(Algebra::Bgp(vec![TriplePattern {
                subject: prev.clone(),
                predicate: rdf_rest.clone(),
                object: rest_object,
            }]));
        }
        Ok(head)
    }

    /// Verb of a blank node property list: `a`, a plain IRI/variable, or a
    /// property path (`:p|:q`, `^:r`, `!(:a|:b)`, `:p*` ...). Returns the
    /// simple verb when the predicate is a plain term, else `(rdf:type, path)`.
    fn parse_verb_or_path(
        &mut self,
    ) -> Result<(TermPattern, Option<PathExpression>), OntolithError> {
        self.skip();
        if self.eat_keyword("a") {
            return Ok((
                TermPattern::Iri(Iri::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type")),
                None,
            ));
        }
        let save = self.checkpoint();
        if matches!(self.peek_char(), Some('^') | Some('!') | Some('(')) {
            self.restore(save);
            let path = self.parse_path_alternative()?;
            return Ok((
                TermPattern::Iri(Iri::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type")),
                Some(path),
            ));
        }
        let simple = self.parse_var_or_term(false)?;
        let is_path = match &simple {
            TermPattern::Iri(_) | TermPattern::Variable(_) => {
                let adjacent_modifier =
                    matches!(self.peek_char(), Some('?') | Some('*') | Some('+'));
                self.skip();
                matches!(self.peek_char(), Some('/') | Some('|')) || adjacent_modifier
            }
            _ => false,
        };
        if !is_path {
            return Ok((simple, None));
        }
        self.restore(save);
        let path = self.parse_path_alternative()?;
        Ok((
            TermPattern::Iri(Iri::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type")),
            Some(path),
        ))
    }

    fn parse_var_or_term(
        &mut self,
        allow_blank_as_var: bool,
    ) -> Result<TermPattern, OntolithError> {
        self.skip();
        if self.peek_char() == Some('?') || self.peek_char() == Some('$') {
            return Ok(TermPattern::Variable(self.parse_var_name()?));
        }
        if self.input[self.pos..].starts_with("_:") {
            let label = self.parse_blank_label()?;
            // Blank labels are existential vars in BGP matching for R1.
            let _ = allow_blank_as_var;
            return Ok(TermPattern::Blank(label));
        }
        if self.peek_char() == Some('[') {
            // Blank node property list `[ p o ; ... ]` — anonymous, unique per
            // occurrence; expansion triples are queued into `pending_group`.
            return self.parse_bnode_property_list();
        }
        if self.peek_char() == Some('(') {
            // RDF collection `( item ... )` — expands into rdf:first/rest
            // triples queued into `pending_group`; returns the head term.
            return self.parse_collection();
        }
        if self.input[self.pos..].starts_with("node:") {
            let start = self.pos + 5;
            self.pos = start;
            self.col += 5;
            let num = self.parse_usize()?;
            return Ok(TermPattern::Node(ontolith_core::domain::NodeId::new(
                num as u64,
            )));
        }
        self.parse_graph_term()
    }

    /// Accept a SPARQL IRIREF. IRIREFs may be relative references (resolved
    /// against the active base by [`Self::parse_iriref`]); with no base they
    /// are kept verbatim, so only empty/whitespace forms are rejected.
    fn iri_term(&self, value: String) -> Result<TermPattern, OntolithError> {
        if value.is_empty() || value.chars().any(|c| c.is_ascii_whitespace()) {
            return Err(self.err("invalid IRI reference"));
        }
        Ok(TermPattern::Iri(Iri::new(value)))
    }

    fn parse_graph_term(&mut self) -> Result<TermPattern, OntolithError> {
        self.skip();
        if self.peek_char() == Some('<') {
            let iri = self.parse_iriref()?;
            return self.iri_term(iri);
        }
        if self.peek_char() == Some('"') || self.peek_char() == Some('\'') {
            return Ok(TermPattern::Literal(self.parse_string_literal()?));
        }
        // number / boolean / node:ID / prefixed name
        let mut word = self.parse_word()?;
        if word.is_empty() {
            return Err(self.err("expected term"));
        }
        // Absorb decimal fraction / exponent after an integer word:
        // `1.5`, `2e3`, `1.5e-2`.
        if is_integer(&word) && matches!(self.peek_char(), Some('.') | Some('e') | Some('E')) {
            while let Some(c) = self.peek_char() {
                if c == '.' || c.is_ascii_digit() || c == 'e' || c == 'E' || c == '+' || c == '-' {
                    self.bump();
                    word.push(c);
                } else {
                    break;
                }
            }
        }
        if let Some(rest) = word.strip_prefix("node:") {
            let id = rest
                .parse::<u64>()
                .map_err(|_| self.err("invalid node id"))?;
            return Ok(TermPattern::Node(ontolith_core::domain::NodeId::new(id)));
        }
        match word.to_ascii_lowercase().as_str() {
            "true" => Ok(TermPattern::Literal(LiteralValue::Boolean(true))),
            "false" => Ok(TermPattern::Literal(LiteralValue::Boolean(false))),
            _ if is_integer(&word) => Ok(TermPattern::Literal(LiteralValue::Integer(
                word.parse().map_err(|_| self.err("bad integer"))?,
            ))),
            _ if is_decimal(&word) => Ok(TermPattern::Literal(LiteralValue::Decimal(
                word.parse().map_err(|_| self.err("bad decimal"))?,
            ))),
            _ if word.contains(':') => {
                let iri = self.expand_prefixed(&word)?;
                Ok(TermPattern::Iri(
                    Iri::parse(iri).map_err(|e| OntolithError::query(e.message()))?,
                ))
            }
            _ => Err(self.err(format!("unexpected term '{word}'"))),
        }
    }

    fn parse_constraint(&mut self) -> Result<Expression, OntolithError> {
        self.skip();
        if self.peek_char() == Some('(') {
            self.bump();
            self.skip();
            let e = self.parse_expression()?;
            self.skip();
            self.expect_char(')')?;
            return Ok(e);
        }
        self.parse_expression()
    }

    fn parse_expression(&mut self) -> Result<Expression, OntolithError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expression, OntolithError> {
        let mut left = self.parse_and()?;
        self.skip();
        while self.eat_keyword("||") || self.eat_keyword("OR") {
            self.skip();
            let right = self.parse_and()?;
            left = Expression::Or(Box::new(left), Box::new(right));
            self.skip();
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expression, OntolithError> {
        let mut left = self.parse_relational()?;
        self.skip();
        while self.eat_keyword("&&") || self.eat_keyword("AND") {
            self.skip();
            let right = self.parse_relational()?;
            left = Expression::And(Box::new(left), Box::new(right));
            self.skip();
        }
        Ok(left)
    }

    fn parse_relational(&mut self) -> Result<Expression, OntolithError> {
        let left = self.parse_additive()?;
        self.skip();
        if self.eat_operator("=") {
            self.skip();
            Ok(Expression::Equal(
                Box::new(left),
                Box::new(self.parse_additive()?),
            ))
        } else if self.eat_operator("!=") {
            self.skip();
            Ok(Expression::NotEqual(
                Box::new(left),
                Box::new(self.parse_additive()?),
            ))
        } else if self.eat_operator("<=") {
            self.skip();
            Ok(Expression::LessEq(
                Box::new(left),
                Box::new(self.parse_additive()?),
            ))
        } else if self.eat_operator(">=") {
            self.skip();
            Ok(Expression::GreaterEq(
                Box::new(left),
                Box::new(self.parse_additive()?),
            ))
        } else if self.eat_operator("<") {
            self.skip();
            Ok(Expression::Less(
                Box::new(left),
                Box::new(self.parse_additive()?),
            ))
        } else if self.eat_operator(">") {
            self.skip();
            Ok(Expression::Greater(
                Box::new(left),
                Box::new(self.parse_additive()?),
            ))
        } else if self.eat_keyword("IN") {
            self.skip();
            let mut args = vec![left];
            self.expect_char('(')?;
            self.skip();
            if self.peek_char() != Some(')') {
                loop {
                    args.push(self.parse_expression()?);
                    self.skip();
                    if self.eat_operator(",") {
                        continue;
                    }
                    break;
                }
            }
            self.expect_char(')')?;
            Ok(Expression::Function {
                name: "IN".into(),
                args,
            })
        } else if self.eat_keyword("NOT") {
            self.skip();
            self.expect_keyword("IN")?;
            self.skip();
            let mut args = vec![left];
            self.expect_char('(')?;
            self.skip();
            if self.peek_char() != Some(')') {
                loop {
                    args.push(self.parse_expression()?);
                    self.skip();
                    if self.eat_operator(",") {
                        continue;
                    }
                    break;
                }
            }
            self.expect_char(')')?;
            Ok(Expression::Function {
                name: "NOT IN".into(),
                args,
            })
        } else {
            Ok(left)
        }
    }

    fn parse_additive(&mut self) -> Result<Expression, OntolithError> {
        let mut left = self.parse_multiplicative()?;
        self.skip();
        loop {
            let op = if self.eat_operator("+") {
                Some('+')
            } else if self.eat_operator("-") {
                Some('-')
            } else {
                None
            };
            let Some(op) = op else { break };
            self.skip();
            let right = self.parse_multiplicative()?;
            left = Expression::Arith {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
            self.skip();
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expression, OntolithError> {
        let mut left = self.parse_unary()?;
        self.skip();
        loop {
            let op = if self.eat_operator("*") {
                Some('*')
            } else if self.eat_operator("/") {
                Some('/')
            } else {
                None
            };
            let Some(op) = op else { break };
            self.skip();
            let right = self.parse_unary()?;
            left = Expression::Arith {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
            self.skip();
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expression, OntolithError> {
        self.skip();
        if self.eat_operator("!") || self.eat_keyword("NOT") {
            self.skip();
            if self.eat_keyword("EXISTS") {
                let pattern = self.parse_group_graph_pattern()?;
                return Ok(Expression::Exists {
                    negated: true,
                    pattern: Box::new(pattern),
                });
            }
            self.skip();
            return Ok(Expression::Not(Box::new(self.parse_unary()?)));
        }
        if self.eat_keyword("EXISTS") {
            let pattern = self.parse_group_graph_pattern()?;
            return Ok(Expression::Exists {
                negated: false,
                pattern: Box::new(pattern),
            });
        }
        if self.peek_char() == Some('-') || self.peek_char() == Some('+') {
            self.bump();
            self.skip();
            return Ok(Expression::Negate(Box::new(self.parse_unary()?)));
        }
        if self.eat_keyword("BOUND") {
            self.skip();
            self.expect_char('(')?;
            self.skip();
            let v = self.parse_var_name()?;
            self.skip();
            self.expect_char(')')?;
            return Ok(Expression::Bound(v));
        }
        if self.eat_keyword("isIRI") || self.eat_keyword("isURI") {
            self.skip();
            self.expect_char('(')?;
            self.skip();
            let e = self.parse_expression()?;
            self.skip();
            self.expect_char(')')?;
            return Ok(Expression::IsIri(Box::new(e)));
        }
        if self.eat_keyword("isLiteral") {
            self.skip();
            self.expect_char('(')?;
            self.skip();
            let e = self.parse_expression()?;
            self.skip();
            self.expect_char(')')?;
            return Ok(Expression::IsLiteral(Box::new(e)));
        }
        if self.eat_keyword("isBlank") {
            self.skip();
            self.expect_char('(')?;
            self.skip();
            let e = self.parse_expression()?;
            self.skip();
            self.expect_char(')')?;
            return Ok(Expression::IsBlank(Box::new(e)));
        }
        if self.eat_keyword("CAST") {
            self.skip();
            self.expect_char('(')?;
            self.skip();
            let expr = self.parse_expression()?;
            self.skip();
            self.expect_keyword("AS")?;
            self.skip();
            let datatype = match self.parse_graph_term()? {
                TermPattern::Iri(i) => i,
                _ => return Err(self.err("CAST target must be a datatype IRI")),
            };
            self.skip();
            self.expect_char(')')?;
            return Ok(Expression::Function {
                name: "CAST".into(),
                args: vec![expr, Expression::Iri(datatype)],
            });
        }
        if self.looking_at_keyword("COUNT")
            || self.looking_at_keyword("SUM")
            || self.looking_at_keyword("AVG")
            || self.looking_at_keyword("MIN")
            || self.looking_at_keyword("MAX")
            || self.looking_at_keyword("GROUP_CONCAT")
            || self.looking_at_keyword("SAMPLE")
        {
            return Ok(Expression::Aggregate(self.parse_aggregate_call()?));
        }
        // Built-in function call: bare name immediately followed by `(`.
        if let Some(name) = self.peek_bare_function_name() {
            self.eat_bare_name();
            self.skip();
            self.expect_char('(')?;
            self.skip();
            let mut args = Vec::new();
            if self.peek_char() != Some(')') {
                loop {
                    args.push(self.parse_expression()?);
                    self.skip();
                    if self.eat_operator(",") {
                        continue;
                    }
                    break;
                }
            }
            self.expect_char(')')?;
            return Ok(Expression::Function {
                name: name.to_ascii_uppercase(),
                args,
            });
        }
        if self.peek_char() == Some('(') {
            self.bump();
            self.skip();
            let e = self.parse_expression()?;
            self.skip();
            self.expect_char(')')?;
            return Ok(e);
        }
        if self.peek_char() == Some('?') || self.peek_char() == Some('$') {
            return Ok(Expression::Variable(self.parse_var_name()?));
        }
        // term
        match self.parse_graph_term()? {
            TermPattern::Iri(i) => Ok(Expression::Iri(i)),
            TermPattern::Literal(l) => Ok(Expression::Literal(l)),
            TermPattern::Variable(v) => Ok(Expression::Variable(v)),
            other => Err(self.err(format!("expression cannot use {other:?}"))),
        }
    }

    // ---- lexer helpers ----

    fn checkpoint(&self) -> (usize, usize, usize, usize) {
        (self.pos, self.line, self.col, self.pending_group.len())
    }

    fn restore(&mut self, c: (usize, usize, usize, usize)) {
        self.pos = c.0;
        self.line = c.1;
        self.col = c.2;
        self.pending_group.truncate(c.3);
    }

    fn eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.pos += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn skip(&mut self) {
        loop {
            while let Some(c) = self.peek_char() {
                if c.is_whitespace() {
                    self.bump();
                } else {
                    break;
                }
            }
            // line comments # ... (SPARQL also supports #)
            if self.peek_char() == Some('#') {
                while let Some(c) = self.peek_char() {
                    self.bump();
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }
            break;
        }
    }

    fn looking_at_keyword(&self, kw: &str) -> bool {
        let rest = &self.input[self.pos..];
        if rest.len() < kw.len() {
            return false;
        }
        let Some(head) = rest.get(..kw.len()) else {
            // kw.len() lands inside a multi-byte char; keywords are ASCII so
            // the prefix cannot match.
            return false;
        };
        if !head.eq_ignore_ascii_case(kw) {
            return false;
        }
        let after = rest[kw.len()..].chars().next();
        after.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
    }

    fn eat_keyword(&mut self, kw: &str) -> bool {
        self.skip();
        if self.looking_at_keyword(kw) {
            for _ in 0..kw.len() {
                self.bump();
            }
            true
        } else {
            false
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), OntolithError> {
        if self.eat_keyword(kw) {
            Ok(())
        } else {
            Err(self.err(format!("expected keyword {kw}")))
        }
    }

    fn eat_operator(&mut self, op: &str) -> bool {
        self.skip();
        if self.input[self.pos..].starts_with(op) {
            for _ in 0..op.len() {
                self.bump();
            }
            true
        } else {
            false
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<(), OntolithError> {
        self.skip();
        if self.peek_char() == Some(expected) {
            self.bump();
            Ok(())
        } else {
            Err(self.err(format!("expected '{expected}'")))
        }
    }

    fn parse_var_name(&mut self) -> Result<String, OntolithError> {
        self.skip();
        let sigil = self.peek_char();
        if sigil != Some('?') && sigil != Some('$') {
            return Err(self.err("expected variable"));
        }
        self.bump();
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.bump();
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(self.err("empty variable name"));
        }
        Ok(self.input[start..self.pos].to_owned())
    }

    /// Peek at a bare (non-prefixed) identifier followed by `(` — a built-in
    /// function call. Returns `None` when the next token is not a function.
    fn peek_bare_function_name(&self) -> Option<String> {
        let mut pos = self.pos;
        while let Some(c) = self.input[pos..].chars().next() {
            if c.is_whitespace() {
                pos += c.len_utf8();
            } else {
                break;
            }
        }
        let mut name = String::new();
        match self.input[pos..].chars().next()? {
            c if c.is_ascii_alphabetic() => {
                name.push(c);
                pos += c.len_utf8();
            }
            ':' => {
                // Empty-prefix function names (`:fn(...)`).
                name.push(':');
                pos += 1;
            }
            _ => return None,
        }
        while let Some(c) = self.input[pos..].chars().next() {
            if c.is_ascii_alphanumeric() || c == '_' {
                name.push(c);
                pos += c.len_utf8();
            } else {
                break;
            }
        }
        // Prefixed function names: `xsd:integer`, `ex:fn`.
        if self.input[pos..].starts_with(':') {
            let p2 = pos + 1;
            if self.input[p2..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            {
                name.push(':');
                pos = p2;
                while let Some(c) = self.input[pos..].chars().next() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        name.push(c);
                        pos += c.len_utf8();
                    } else {
                        break;
                    }
                }
            }
        }
        // Skip whitespace between the name and `(`.
        let mut rest = &self.input[pos..];
        while let Some(c) = rest.chars().next() {
            if c.is_whitespace() {
                pos += c.len_utf8();
                rest = &self.input[pos..];
            } else {
                break;
            }
        }
        if rest.starts_with('(') {
            Some(name)
        } else {
            None
        }
    }

    fn eat_bare_name(&mut self) {
        self.skip();
        while let Some(c) = self.peek_char() {
            if c.is_ascii_alphanumeric() || c == '_' || c == ':' {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn parse_blank_label(&mut self) -> Result<String, OntolithError> {
        // assumes starts with _:
        self.bump();
        self.bump();
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                self.bump();
            } else {
                break;
            }
        }
        Ok(self.input[start..self.pos].to_owned())
    }

    fn parse_iriref(&mut self) -> Result<String, OntolithError> {
        self.skip();
        if self.peek_char() != Some('<') {
            return Err(self.err("expected IRI"));
        }
        self.bump();
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c == '>' {
                let iri = self.input[start..self.pos].to_owned();
                self.bump();
                // SPARQL resolves IRI references against the active base.
                if let Some(base) = &self.base
                    && let Some(resolved) = super::execute::resolve_iri(&iri, base)
                {
                    return Ok(resolved);
                }
                return Ok(iri);
            }
            self.bump();
        }
        Err(self.err("unterminated IRI"))
    }

    fn parse_prefixed_name_left(&mut self) -> Result<String, OntolithError> {
        // pname like ex: or :
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c == ':' {
                let name = self.input[start..self.pos].to_owned();
                self.bump();
                if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    return Err(self.err("prefix name must not start with a digit"));
                }
                return Ok(name);
            }
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                self.bump();
            } else {
                break;
            }
        }
        Err(self.err("expected prefixed name"))
    }

    fn parse_string_literal(&mut self) -> Result<LiteralValue, OntolithError> {
        let quote = self.bump().unwrap();
        let start = self.pos;
        let mut escaped = false;
        while let Some(c) = self.peek_char() {
            if escaped {
                escaped = false;
                self.bump();
                continue;
            }
            if c == '\\' {
                escaped = true;
                self.bump();
                continue;
            }
            if c == quote {
                let raw = self.input[start..self.pos].to_owned();
                self.bump();
                // datatype / lang
                if self.peek_char() == Some('@') {
                    self.bump();
                    let lang_start = self.pos;
                    while self
                        .peek_char()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
                    {
                        self.bump();
                    }
                    let lang = self.input[lang_start..self.pos].to_ascii_lowercase();
                    let lang = LanguageTag::parse(lang).map_err(|e| self.err(e.message()))?;
                    self.check_unicode_escapes(&raw)?;
                    return Ok(LiteralValue::Lang {
                        value: unescape(&raw),
                        lang,
                    });
                }
                if self.input[self.pos..].starts_with("^^") {
                    self.bump();
                    self.bump();
                    let dt = if self.peek_char() == Some('<') {
                        self.parse_iriref()?
                    } else {
                        let w = self.parse_word()?;
                        self.expand_prefixed(&w)?
                    };
                    self.check_unicode_escapes(&raw)?;
                    return Ok(coerce_literal(unescape(&raw), &dt));
                }
                self.check_unicode_escapes(&raw)?;
                return Ok(LiteralValue::String(unescape(&raw)));
            }
            self.bump();
        }
        Err(self.err("unterminated string"))
    }

    fn parse_word(&mut self) -> Result<String, OntolithError> {
        // PN_LOCAL_ESC characters that may follow a backslash in a prefixed
        // name local part (e.g. `:d\?` is the IRI char `?`).
        const PN_LOCAL_ESC: &[char] = &[
            '_', '~', '.', '-', '!', '$', '&', '\'', '(', ')', '*', '+', ',', ';', '=', '/', '?',
            '#', '@', '%',
        ];
        let mut out = String::new();
        while let Some(c) = self.peek_char() {
            if c == '\\' {
                let escaped = self.input[self.pos..].chars().nth(1);
                if let Some(esc) = escaped.filter(|e| PN_LOCAL_ESC.contains(e)) {
                    self.bump();
                    self.bump();
                    out.push(esc);
                    continue;
                }
                return Err(self.err("invalid escape sequence in prefixed name"));
            }
            if c.is_whitespace()
                || matches!(
                    c,
                    '{' | '}'
                        | '('
                        | ')'
                        | ';'
                        | ','
                        | '<'
                        | '"'
                        | '\''
                        | '#'
                        | '!'
                        | '='
                        | '>'
                        | '/'
                        | '^'
                        | '+'
                        | '*'
                        | '&'
                        | '|'
                        | '?'
                )
                || (c == '.'
                    && self.input[self.pos + 1..].chars().next().is_none_or(|n| {
                        n.is_whitespace()
                            || matches!(
                                n,
                                '{' | '}'
                                    | '('
                                    | ')'
                                    | '.'
                                    | ';'
                                    | ','
                                    | '<'
                                    | '"'
                                    | '\''
                                    | '#'
                                    | '!'
                                    | '='
                                    | '>'
                                    | '/'
                                    | '^'
                                    | '+'
                                    | '*'
                                    | '&'
                                    | '|'
                                    | '?'
                            )
                    }))
            {
                break;
            }
            self.bump();
            out.push(c);
        }
        Ok(out)
    }

    fn parse_usize(&mut self) -> Result<usize, OntolithError> {
        self.skip();
        let start = self.pos;
        while self.peek_char().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        if self.pos == start {
            return Err(self.err("expected integer"));
        }
        self.input[start..self.pos]
            .parse()
            .map_err(|_| self.err("bad integer"))
    }

    fn expand_prefixed(&self, token: &str) -> Result<String, OntolithError> {
        if let Some((p, local)) = token.split_once(':') {
            if let Some(ns) = self.prefixes.get(p) {
                return Ok(format!("{ns}{local}"));
            }
            return Err(OntolithError::query(format!("unknown prefix '{p}'")));
        }
        Ok(token.to_owned())
    }

    /// Validates `\uXXXX` / `\UXXXXXXXX` escapes inside a string literal raw
    /// body. A code point in the surrogate range U+D800..U+DFFF cannot be
    /// represented by a Unicode escape in SPARQL (W3C
    /// syn-invalid-codepoint-escaped-bad-01), and an escape with fewer digits
    /// than the grammar requires is a syntax error.
    fn check_unicode_escapes(&self, raw: &str) -> Result<(), OntolithError> {
        let bytes = raw.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'\\' {
                i += 1;
                continue;
            }
            match bytes.get(i + 1).copied() {
                Some(b'u') | Some(b'U') => {
                    let ndigits = if bytes[i + 1] == b'u' { 4 } else { 8 };
                    let hex_start = i + 2;
                    if hex_start + ndigits > bytes.len() {
                        return Err(self.err("incomplete unicode escape in string literal"));
                    }
                    let hex = &raw[hex_start..hex_start + ndigits];
                    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err(self.err("malformed unicode escape in string literal"));
                    }
                    let cp = u32::from_str_radix(hex, 16)
                        .map_err(|_| self.err("malformed unicode escape in string literal"))?;
                    if (0xD800..=0xDFFF).contains(&cp) {
                        return Err(self.err("surrogate codepoint in string literal escape"));
                    }
                    i = hex_start + ndigits;
                }
                Some(_) => i += 2,
                None => i += 1,
            }
        }
        Ok(())
    }

    fn err(&self, msg: impl Into<String>) -> OntolithError {
        OntolithError::parse_at(self.line, self.col, msg.into())
    }
}

fn update_pattern_blanks(patterns: &[UpdatePattern]) -> Vec<String> {
    let mut out = Vec::new();
    for p in patterns {
        if let TermPattern::Blank(label) = &p.triple.subject {
            out.push(label.clone());
        }
        if let TermPattern::Blank(label) = &p.triple.object {
            out.push(label.clone());
        }
    }
    out
}

/// Blank labels used by INSERT/DELETE DATA operations. Labels in modify
/// templates are operation-scoped variables and are not tracked here.
fn data_op_blank_labels(op: &UpdateOp) -> Vec<String> {
    match op {
        UpdateOp::InsertData(p) | UpdateOp::DeleteData(p) => update_pattern_blanks(p),
        _ => Vec::new(),
    }
}

fn join(left: Algebra, right: Algebra) -> Algebra {
    match left {
        Algebra::Identity => right,
        other => Algebra::Join {
            left: Box::new(other),
            right: Box::new(right),
        },
    }
}

/// Specialize first unbound subject variable in BGP tree to `node`.
fn apply_subject_hint(algebra: &mut Algebra, node: ontolith_core::domain::NodeId) -> bool {
    match algebra {
        Algebra::Bgp(patterns) => {
            for p in patterns.iter_mut() {
                if p.subject.is_variable() {
                    p.subject = TermPattern::Node(node);
                    return true;
                }
            }
            false
        }
        Algebra::Join { left, right }
        | Algebra::LeftJoin { left, right, .. }
        | Algebra::Union { left, right } => {
            apply_subject_hint(left, node) || apply_subject_hint(right, node)
        }
        Algebra::Filter { input, .. }
        | Algebra::Extend { input, .. }
        | Algebra::Distinct { input }
        | Algebra::Project { input, .. }
        | Algebra::OrderBy { input, .. }
        | Algebra::Slice { input, .. }
        | Algebra::Aggregate { input, .. } => apply_subject_hint(input, node),
        Algebra::Graph { inner, .. } => apply_subject_hint(inner, node),
        Algebra::Path { subject, .. } if subject.is_variable() => {
            *subject = TermPattern::Node(node);
            true
        }
        Algebra::Path { .. } => false,
        _ => false,
    }
}

fn algebra_tag(a: &Algebra) -> &'static str {
    match a {
        Algebra::Bgp(_) => "bgp",
        Algebra::Join { .. } => "join",
        Algebra::LeftJoin { .. } => "leftjoin",
        Algebra::Union { .. } => "union",
        Algebra::Filter { .. } => "filter",
        Algebra::Aggregate { .. } => "aggregate",
        Algebra::Path { .. } => "path",
        Algebra::Graph { .. } => "graph",
        Algebra::Identity => "identity",
        _ => "algebra",
    }
}

pub fn physical_steps_public(algebra: &Algebra) -> Vec<String> {
    physical_steps(algebra)
}

fn physical_steps(algebra: &Algebra) -> Vec<String> {
    let mut steps = Vec::new();
    walk_physical(algebra, &mut steps);
    steps
}

fn walk_physical(algebra: &Algebra, steps: &mut Vec<String>) {
    match algebra {
        Algebra::Identity => steps.push("identity".into()),
        Algebra::Bgp(p) => {
            if let Some(s) = p.first() {
                if let TermPattern::Node(n) = &s.subject {
                    steps.push(format!("index_spo:{}", n.get()));
                } else if let TermPattern::Iri(i) = &s.predicate {
                    steps.push(format!("index_pos:{}", i.as_str()));
                } else if let TermPattern::Iri(i) = &s.object {
                    steps.push(format!("index_osp:{}", i.as_str()));
                } else {
                    steps.push(format!("bgp_scan:{}", p.len()));
                }
            } else {
                steps.push("bgp_empty".into());
            }
        }
        Algebra::Join { left, right } => {
            walk_physical(left, steps);
            walk_physical(right, steps);
            steps.push("hash_join".into());
        }
        Algebra::LeftJoin { left, right, .. } => {
            walk_physical(left, steps);
            walk_physical(right, steps);
            steps.push("left_join".into());
        }
        Algebra::Union { left, right } => {
            walk_physical(left, steps);
            walk_physical(right, steps);
            steps.push("union".into());
        }
        Algebra::Minus { left, right } => {
            walk_physical(left, steps);
            walk_physical(right, steps);
            steps.push("minus".into());
        }
        Algebra::Filter { input, .. } => {
            walk_physical(input, steps);
            steps.push("filter".into());
        }
        Algebra::Extend { input, .. } => {
            walk_physical(input, steps);
            steps.push("extend".into());
        }
        Algebra::Values { bindings, .. } => steps.push(format!("values:{}", bindings.len())),
        Algebra::Distinct { input } => {
            walk_physical(input, steps);
            steps.push("distinct".into());
        }
        Algebra::Project { input, .. } => {
            walk_physical(input, steps);
            steps.push("project".into());
        }
        Algebra::OrderBy { input, .. } => {
            walk_physical(input, steps);
            steps.push("order_by".into());
        }
        Algebra::Slice {
            offset,
            limit,
            input,
        } => {
            walk_physical(input, steps);
            steps.push(format!("slice:{offset}:{limit:?}"));
        }
        Algebra::Aggregate {
            groups,
            aggregates,
            having,
            input,
        } => {
            walk_physical(input, steps);
            if !groups.is_empty() {
                steps.push(format!("group_by:{}", groups.join(",")));
            }
            for spec in aggregates {
                let fun = match &spec.function {
                    AggregateFunction::Count {
                        expr: None,
                        distinct: false,
                    } => "COUNT(*)".to_string(),
                    AggregateFunction::Count {
                        expr: Some(expr),
                        distinct: false,
                    } => format!("COUNT({})", summarize_agg_expr(expr)),
                    AggregateFunction::Count {
                        expr: Some(expr),
                        distinct: true,
                    } => format!("COUNT(DISTINCT {})", summarize_agg_expr(expr)),
                    AggregateFunction::Count {
                        expr: None,
                        distinct: true,
                    } => "COUNT(DISTINCT *)".to_string(),
                    AggregateFunction::Sum { expr, .. } => {
                        format!("SUM({})", summarize_agg_expr(expr))
                    }
                    AggregateFunction::Avg { expr, .. } => {
                        format!("AVG({})", summarize_agg_expr(expr))
                    }
                    AggregateFunction::Min { expr, .. } => {
                        format!("MIN({})", summarize_agg_expr(expr))
                    }
                    AggregateFunction::Max { expr, .. } => {
                        format!("MAX({})", summarize_agg_expr(expr))
                    }
                    AggregateFunction::GroupConcat { expr, distinct, .. } => {
                        if *distinct {
                            format!("GROUP_CONCAT(DISTINCT {})", summarize_agg_expr(expr))
                        } else {
                            format!("GROUP_CONCAT({})", summarize_agg_expr(expr))
                        }
                    }
                    AggregateFunction::Sample { expr } => {
                        format!("SAMPLE({})", summarize_agg_expr(expr))
                    }
                };
                steps.push(format!("{fun}->?{}", spec.output));
            }
            if having.is_some() {
                steps.push("having".into());
            }
        }
        Algebra::Path { path, .. } => {
            steps.push(format!("property_path:{path:?}"));
        }
        Algebra::Graph { .. } => {
            steps.push("graph".into());
        }
    }
}

fn summarize_agg_expr(expr: &AggregateExpr) -> String {
    match expr {
        AggregateExpr::Variable(v) => format!("?{v}"),
        AggregateExpr::Expression(e) => format!("{e:?}"),
    }
}

pub fn parse_subject_hint(
    query: &str,
) -> Result<Option<ontolith_core::domain::NodeId>, OntolithError> {
    let normalized = query.to_ascii_lowercase();
    let marker = "subject=";
    let Some(marker_pos) = normalized.find(marker) else {
        return Ok(None);
    };
    let start = marker_pos + marker.len();
    let rest = &normalized[start..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return Err(OntolithError::InvalidState("invalid subject hint"));
    }
    let value = digits
        .parse::<u64>()
        .map_err(|_| OntolithError::InvalidState("invalid subject hint"))?;
    Ok(Some(ontolith_core::domain::NodeId::new(value)))
}

pub fn plan_id(query: &str) -> QueryPlanId {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in query.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    QueryPlanId(hash)
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        let next = bytes.get(i + 1).copied();
        match next {
            Some(b'u') | Some(b'U') => {
                let ndigits = if next == Some(b'u') { 4 } else { 8 };
                let hex_start = i + 2;
                if hex_start + ndigits <= bytes.len() {
                    let hex = &s[hex_start..hex_start + ndigits];
                    if let Some(ch) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                        i = hex_start + ndigits;
                        continue;
                    }
                }
                out.push('\\');
                i += 1;
            }
            Some(b'n') => {
                out.push('\n');
                i += 2;
            }
            Some(b't') => {
                out.push('\t');
                i += 2;
            }
            Some(b'r') => {
                out.push('\r');
                i += 2;
            }
            Some(_) => {
                let ch = s[i + 1..].chars().next().unwrap();
                out.push(ch);
                i += 1 + ch.len_utf8();
            }
            None => {
                out.push('\\');
                i += 1;
            }
        }
    }
    out
}

fn coerce_literal(content: String, dt: &str) -> LiteralValue {
    match dt {
        "http://www.w3.org/2001/XMLSchema#string" => LiteralValue::String(content),
        "http://www.w3.org/2001/XMLSchema#integer"
        | "http://www.w3.org/2001/XMLSchema#int"
        | "http://www.w3.org/2001/XMLSchema#long" => content
            .parse()
            .map(LiteralValue::Integer)
            .unwrap_or_else(|_| typed(content, dt)),
        "http://www.w3.org/2001/XMLSchema#decimal" => content
            .parse()
            .map(LiteralValue::Decimal)
            .unwrap_or_else(|_| typed(content, dt)),
        "http://www.w3.org/2001/XMLSchema#float" => content
            .parse()
            .map(LiteralValue::Float)
            .unwrap_or_else(|_| typed(content, dt)),
        "http://www.w3.org/2001/XMLSchema#double" => content
            .parse()
            .map(LiteralValue::Double)
            .unwrap_or_else(|_| typed(content, dt)),
        "http://www.w3.org/2001/XMLSchema#boolean" => match content.as_str() {
            // RDF 1.1 term distinctness: `"1"^^xsd:boolean` != `true^^xsd:boolean`.
            "true" => LiteralValue::Boolean(true),
            "false" => LiteralValue::Boolean(false),
            _ => typed(content, dt),
        },
        _ => typed(content, dt),
    }
}

fn typed(content: String, datatype: &str) -> LiteralValue {
    LiteralValue::Typed {
        value: content,
        datatype: Iri::new(datatype),
    }
}

fn is_integer(s: &str) -> bool {
    let t = s.trim_start_matches(['+', '-']);
    !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())
}

fn is_decimal(s: &str) -> bool {
    s.parse::<f64>().is_ok() && s.chars().any(|c| c == '.' || c == 'e' || c == 'E')
}

/// Rewrites aggregate function calls nested anywhere in an expression to the
/// output alias of the matching aggregate. Aggregates not present in the
/// projection (e.g. a bare `COUNT(*)` inside HAVING) are appended to the
/// aggregate list with a synthetic output variable so the executor computes
/// them per group and the rewritten expression can read the bound result.
fn lift_aggregates(
    expr: Expression,
    aggregates: &mut Vec<AggregateSpec>,
) -> Result<Expression, OntolithError> {
    Ok(match expr {
        Expression::Aggregate(function) => {
            if let Some(spec) = aggregates.iter().find(|s| s.function == function) {
                Expression::Variable(spec.output.clone())
            } else {
                let output = format!("__agg_{}", aggregates.len());
                aggregates.push(AggregateSpec {
                    function,
                    output: output.clone(),
                });
                Expression::Variable(output)
            }
        }
        Expression::Not(e) => Expression::Not(Box::new(lift_aggregates(*e, aggregates)?)),
        Expression::And(a, b) => Expression::And(
            Box::new(lift_aggregates(*a, aggregates)?),
            Box::new(lift_aggregates(*b, aggregates)?),
        ),
        Expression::Or(a, b) => Expression::Or(
            Box::new(lift_aggregates(*a, aggregates)?),
            Box::new(lift_aggregates(*b, aggregates)?),
        ),
        Expression::Equal(a, b) => Expression::Equal(
            Box::new(lift_aggregates(*a, aggregates)?),
            Box::new(lift_aggregates(*b, aggregates)?),
        ),
        Expression::NotEqual(a, b) => Expression::NotEqual(
            Box::new(lift_aggregates(*a, aggregates)?),
            Box::new(lift_aggregates(*b, aggregates)?),
        ),
        Expression::Less(a, b) => Expression::Less(
            Box::new(lift_aggregates(*a, aggregates)?),
            Box::new(lift_aggregates(*b, aggregates)?),
        ),
        Expression::LessEq(a, b) => Expression::LessEq(
            Box::new(lift_aggregates(*a, aggregates)?),
            Box::new(lift_aggregates(*b, aggregates)?),
        ),
        Expression::Greater(a, b) => Expression::Greater(
            Box::new(lift_aggregates(*a, aggregates)?),
            Box::new(lift_aggregates(*b, aggregates)?),
        ),
        Expression::GreaterEq(a, b) => Expression::GreaterEq(
            Box::new(lift_aggregates(*a, aggregates)?),
            Box::new(lift_aggregates(*b, aggregates)?),
        ),
        Expression::Arith { op, left, right } => Expression::Arith {
            op,
            left: Box::new(lift_aggregates(*left, aggregates)?),
            right: Box::new(lift_aggregates(*right, aggregates)?),
        },
        Expression::Negate(e) => Expression::Negate(Box::new(lift_aggregates(*e, aggregates)?)),
        Expression::IsIri(e) => Expression::IsIri(Box::new(lift_aggregates(*e, aggregates)?)),
        Expression::IsLiteral(e) => {
            Expression::IsLiteral(Box::new(lift_aggregates(*e, aggregates)?))
        }
        Expression::IsBlank(e) => Expression::IsBlank(Box::new(lift_aggregates(*e, aggregates)?)),
        Expression::Function { name, args } => Expression::Function {
            name,
            args: args
                .into_iter()
                .map(|a| lift_aggregates(a, aggregates))
                .collect::<Result<Vec<_>, _>>()?,
        },
        other => other,
    })
}

/// Variables already bound by an algebra subtree (for BIND scope checks).
fn bindings_in_algebra(algebra: &Algebra) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_algebra_vars(algebra, &mut out);
    out
}

fn collect_algebra_vars(algebra: &Algebra, out: &mut BTreeSet<String>) {
    match algebra {
        Algebra::Identity => {}
        Algebra::Bgp(patterns) => {
            for p in patterns {
                for t in [&p.subject, &p.predicate, &p.object] {
                    if let Some(v) = t.as_variable() {
                        out.insert(v.to_owned());
                    }
                }
            }
        }
        Algebra::Join { left, right } => {
            collect_algebra_vars(left, out);
            collect_algebra_vars(right, out);
        }
        Algebra::LeftJoin {
            left,
            right,
            condition,
        } => {
            collect_algebra_vars(left, out);
            collect_algebra_vars(right, out);
            if let Some(expr) = condition {
                collect_expr_vars(expr, out);
            }
        }
        Algebra::Union { left, right } => {
            collect_algebra_vars(left, out);
            collect_algebra_vars(right, out);
        }
        Algebra::Minus { left, right } => {
            collect_algebra_vars(left, out);
            collect_algebra_vars(right, out);
        }
        Algebra::Filter { expression, input } => {
            // Filter expressions reference variables without binding them, so
            // they do not count toward BIND scope checks.
            let _ = expression;
            collect_algebra_vars(input, out);
        }
        Algebra::Extend {
            variable,
            expression,
            input,
        } => {
            out.insert(variable.clone());
            collect_expr_vars(expression, out);
            collect_algebra_vars(input, out);
        }
        Algebra::Values {
            variables,
            bindings,
            ..
        } => {
            out.extend(variables.iter().cloned());
            for row in bindings {
                for t in row {
                    if let Some(v) = t.as_ref().and_then(|t| t.as_variable()) {
                        out.insert(v.to_owned());
                    }
                }
            }
        }
        Algebra::Distinct { input }
        | Algebra::Project { input, .. }
        | Algebra::OrderBy { input, .. }
        | Algebra::Slice { input, .. }
        | Algebra::Aggregate { input, .. } => collect_algebra_vars(input, out),
        Algebra::Path {
            subject, object, ..
        } => {
            if let Some(v) = subject.as_variable() {
                out.insert(v.to_owned());
            }
            if let Some(v) = object.as_variable() {
                out.insert(v.to_owned());
            }
        }
        Algebra::Graph { graph, inner } => {
            if let Some(v) = graph.as_variable() {
                out.insert(v.to_owned());
            }
            collect_algebra_vars(inner, out);
        }
    }
}

fn collect_expr_vars(expr: &Expression, out: &mut BTreeSet<String>) {
    match expr {
        Expression::Variable(v) => {
            out.insert(v.clone());
        }
        Expression::Bound(v) => {
            out.insert(v.clone());
        }
        Expression::Exists { pattern, .. } => {
            collect_algebra_vars(pattern, out);
        }
        Expression::Iri(_) | Expression::Literal(_) | Expression::Aggregate(_) => {}
        Expression::IsIri(e)
        | Expression::IsLiteral(e)
        | Expression::IsBlank(e)
        | Expression::Not(e)
        | Expression::Negate(e) => collect_expr_vars(e, out),
        Expression::And(a, b)
        | Expression::Or(a, b)
        | Expression::Equal(a, b)
        | Expression::NotEqual(a, b)
        | Expression::Less(a, b)
        | Expression::LessEq(a, b)
        | Expression::Greater(a, b)
        | Expression::GreaterEq(a, b)
        | Expression::Arith {
            left: a, right: b, ..
        } => {
            collect_expr_vars(a, out);
            collect_expr_vars(b, out);
        }
        Expression::Function { args, .. } => {
            for a in args {
                collect_expr_vars(a, out);
            }
        }
    }
}

/// Mirror a path expression: `^(p/q)` becomes `^q/^p`, `!(p1|^p2)` becomes
/// `!(^p1|p2)`, and modifiers carry through.
fn invert_path(path: PathExpression) -> PathExpression {
    match path {
        PathExpression::Predicate(p) => PathExpression::InversePredicate(p),
        PathExpression::InversePredicate(p) => PathExpression::Predicate(p),
        PathExpression::Sequence(a, b) => {
            PathExpression::Sequence(Box::new(invert_path(*b)), Box::new(invert_path(*a)))
        }
        PathExpression::Alternative(a, b) => {
            PathExpression::Alternative(Box::new(invert_path(*a)), Box::new(invert_path(*b)))
        }
        PathExpression::OneOrMore(p) => PathExpression::OneOrMore(Box::new(invert_path(*p))),
        PathExpression::ZeroOrMore(p) => PathExpression::ZeroOrMore(Box::new(invert_path(*p))),
        PathExpression::ZeroOrOne(p) => PathExpression::ZeroOrOne(Box::new(invert_path(*p))),
        PathExpression::NegatedPropertySet { forward, reverse } => {
            PathExpression::NegatedPropertySet {
                forward: reverse,
                reverse: forward,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::QueryText;
    use ontolith_core::domain::ConsistencyLevel;

    fn req(query: &str) -> QueryRequest {
        QueryRequest {
            query: QueryText(query.to_string()),
            txn_id: None,
            tenant: None,
            tenant_scope: None,
            timeout_ms: None,
            cancel: None,
            consistency: ConsistencyLevel::Strong,
        }
    }

    #[test]
    fn parses_non_ascii_literal_in_filter() {
        // Regression: keyword lookahead must not byte-slice across a
        // multi-byte char, e.g. CONTAINS(STR(?o), "中文").
        let plan = plan_query(&req(
            "PREFIX ex: <http://e/> \
             SELECT ?s WHERE { ?s ex:label ?o . \
             FILTER(CONTAINS(STR(?o), \"中文\")) }",
        ))
        .expect("parse should not panic");
        assert_eq!(plan.kind, QueryKind::Select);
    }
}
