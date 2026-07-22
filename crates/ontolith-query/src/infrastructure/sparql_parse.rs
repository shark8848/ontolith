//! SPARQL 1.1 Query parser (core surface for L3).
//!
//! Produces a [`QueryPlan`] with algebra covering:
//! SELECT / ASK / CONSTRUCT, WHERE groups, OPTIONAL, UNION, FILTER, BIND,
//! VALUES, DISTINCT, ORDER BY, LIMIT/OFFSET, PREFIX/BASE.

use crate::domain::{
    AggregateFunction, Algebra, Expression, OrderKey, QueryKind, QueryPlan, QueryPlanId,
    QueryRequest, TermPattern, TriplePattern,
};
use ontolith_core::domain::{Iri, LiteralValue};
use ontolith_core::error::OntolithError;
use std::collections::BTreeMap;

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
}

struct SelectCountProjection {
    variable: Option<String>,
    output: String,
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
        } else if self.eat_keyword("INSERT")
            || self.eat_keyword("DELETE")
            || self.eat_keyword("WITH")
            || self.eat_keyword("LOAD")
            || self.eat_keyword("CLEAR")
        {
            QueryKind::Update
        } else {
            // Legacy bare patterns / subject= hints still accepted as SELECT.
            QueryKind::Select
        };
        self.logical.push(format!("detect_kind:{}", kind.as_str()));

        if matches!(kind, QueryKind::Update | QueryKind::Describe) {
            return Ok(QueryPlan {
                id: plan_id(self.input),
                kind,
                algebra: Algebra::Identity,
                prefixes: self.prefixes.clone(),
                base: self.base.clone(),
                logical_steps: self.logical.clone(),
                physical_steps: vec![format!("unsupported:{}", kind.as_str())],
                construct_template: Vec::new(),
            });
        }

        let mut distinct = false;
        let mut select_vars: Vec<String> = Vec::new();
        let mut select_count_projection: Option<SelectCountProjection> = None;
        let mut construct_template = Vec::new();

        if kind == QueryKind::Select {
            self.skip();
            if self.eat_keyword("DISTINCT") {
                distinct = true;
                self.logical.push("distinct".into());
            }
            self.skip();
            if self.peek_char() == Some('*') {
                self.bump();
                select_vars.clear();
                self.logical.push("project:*".into());
            } else {
                while self.peek_char() == Some('?') || self.peek_char() == Some('$') {
                    select_vars.push(self.parse_var_name()?);
                    self.skip();
                }
                if select_vars.is_empty() && self.looking_at_keyword("WHERE") {
                    // SELECT WHERE without vars → *
                } else if !select_vars.is_empty() {
                    self.logical
                        .push(format!("project:{}", select_vars.join(",")));
                }
            }
        } else if kind == QueryKind::Construct {
            self.skip();
            if self.peek_char() == Some('{') {
                construct_template = self.parse_construct_template()?;
                self.logical
                    .push(format!("construct_template:{}", construct_template.len()));
            }
        }

        self.skip();
        // WHERE is optional for ASK { } form
        let _ = self.eat_keyword("WHERE");
        self.skip();

        let mut body = if self.peek_char() == Some('{') {
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
                self.skip();
                if self.peek_char() == Some('(') {
                    if !select_vars.is_empty() {
                        return Err(OntolithError::query(
                            "mixed aggregate and non-aggregate projection requires GROUP BY",
                        ));
                    }
                    let projection = self.parse_select_count_projection()?;
                    self.logical
                        .push(format!("aggregate:count:as=?{}", projection.output));
                    select_vars.push(projection.output.clone());
                    select_count_projection = Some(projection);
                    self.skip();
                }
        // Legacy `# subject=N` specializes unbound subjects even when WHERE is present.
        if let Some(hint_subj) = parse_subject_hint(self.input)?
            && apply_subject_hint(&mut body, hint_subj)
        {
            self.logical.push("apply_subject_filter".into());
        }
        self.logical.push(format!("where:{}", algebra_tag(&body)));

            if distinct && select_count_projection.is_some() {
                return Err(OntolithError::query(
                    "DISTINCT with aggregate projection is not yet supported",
                ));
            }

        // solution modifiers
        let mut algebra = body;

        if let Some(aggregate) = select_count_projection {
            algebra = Algebra::Aggregate {
                function: AggregateFunction::Count {
                    variable: aggregate.variable,
                },
                output: aggregate.output,
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

        if kind == QueryKind::Select {
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

        let physical = physical_steps(&algebra);
        Ok(QueryPlan {
            id: plan_id(self.input),
            kind,
            algebra,
            prefixes: self.prefixes.clone(),
            base: self.base.clone(),
            logical_steps: self.logical.clone(),
            physical_steps: physical,
            construct_template,
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
                patterns.push(p);
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

    fn parse_select_count_projection(&mut self) -> Result<SelectCountProjection, OntolithError> {
        self.expect_char('(')?;
        self.skip();
        if !self.eat_keyword("COUNT") {
            return Err(self.err("only COUNT aggregate is currently supported"));
        }
        self.skip();
        self.expect_char('(')?;
        self.skip();

        let variable = if self.peek_char() == Some('*') {
            self.bump();
            None
        } else if self.peek_char() == Some('?') || self.peek_char() == Some('$') {
            Some(self.parse_var_name()?)
        } else {
            return Err(self.err("COUNT expects '*' or variable"));
        };

        self.skip();
        self.expect_char(')')?;
        self.skip();
        self.expect_keyword("AS")?;
        self.skip();
        let output = self.parse_var_name()?;
        self.skip();
        self.expect_char(')')?;

        Ok(SelectCountProjection { variable, output })
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
        self.skip();
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
            } else if self.eat_keyword("FILTER") {
                self.skip();
                let expr = self.parse_constraint()?;
                acc = Algebra::Filter {
                    expression: expr,
                    input: Box::new(acc),
                };
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
            } else if self.looking_at_keyword("SELECT") {
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
            } else if let Some(pattern) = self.try_parse_triple_pattern()? {
                // collect consecutive triple patterns into one BGP
                let mut bgp = vec![pattern];
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
                        bgp.push(p);
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
            } else {
                break;
            }
            self.skip();
        }
        Ok(acc)
    }

    fn parse_subquery_select(&mut self) -> Result<Algebra, OntolithError> {
        self.expect_keyword("SELECT")?;
        self.skip();

        let mut distinct = false;
        let mut select_vars: Vec<String> = Vec::new();

        if self.eat_keyword("DISTINCT") {
            distinct = true;
            self.skip();
        }

        if self.peek_char() == Some('*') {
            self.bump();
            self.skip();
        } else {
            while self.peek_char() == Some('?') || self.peek_char() == Some('$') {
                select_vars.push(self.parse_var_name()?);
                self.skip();
            }
            if select_vars.is_empty() {
                return Err(self.err("subquery SELECT requires '*' or variables"));
            }
        }

        let _ = self.eat_keyword("WHERE");
        self.skip();
        if self.peek_char() != Some('{') {
            return Err(self.err("subquery SELECT requires group graph pattern"));
        }

        let mut algebra = self.parse_group_graph_pattern()?;

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

    fn parse_graph_term(&mut self) -> Result<TermPattern, OntolithError> {
        self.skip();
        if self.peek_char() == Some('<') {
            let iri = self.parse_iriref()?;
            return Ok(TermPattern::Iri(
                Iri::parse(iri).map_err(|e| OntolithError::query(e.message()))?,
            ));
        }
        if self.peek_char() == Some('"') || self.peek_char() == Some('\'') {
            return Ok(TermPattern::Literal(self.parse_string_literal()?));
        }
        // number / boolean / node:ID / prefixed name
        let word = self.parse_word();
        if word.is_empty() {
            return Err(self.err("expected term"));
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
        let left = self.parse_unary()?;
        self.skip();
        if self.eat_operator("=") {
            self.skip();
            Ok(Expression::Equal(
                Box::new(left),
                Box::new(self.parse_unary()?),
            ))
        } else if self.eat_operator("!=") {
            self.skip();
            Ok(Expression::NotEqual(
                Box::new(left),
                Box::new(self.parse_unary()?),
            ))
        } else if self.eat_operator("<=") {
            self.skip();
            Ok(Expression::LessEq(
                Box::new(left),
                Box::new(self.parse_unary()?),
            ))
        } else if self.eat_operator(">=") {
            self.skip();
            Ok(Expression::GreaterEq(
                Box::new(left),
                Box::new(self.parse_unary()?),
            ))
        } else if self.eat_operator("<") {
            self.skip();
            Ok(Expression::Less(
                Box::new(left),
                Box::new(self.parse_unary()?),
            ))
        } else if self.eat_operator(">") {
            self.skip();
            Ok(Expression::Greater(
                Box::new(left),
                Box::new(self.parse_unary()?),
            ))
        } else {
            Ok(left)
        }
    }

    fn parse_unary(&mut self) -> Result<Expression, OntolithError> {
        self.skip();
        if self.eat_operator("!") || self.eat_keyword("NOT") {
            self.skip();
            return Ok(Expression::Not(Box::new(self.parse_unary()?)));
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

    fn checkpoint(&self) -> (usize, usize, usize) {
        (self.pos, self.line, self.col)
    }

    fn restore(&mut self, c: (usize, usize, usize)) {
        self.pos = c.0;
        self.line = c.1;
        self.col = c.2;
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
        if !rest[..kw.len()].eq_ignore_ascii_case(kw) {
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
                    while self
                        .peek_char()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
                    {
                        self.bump();
                    }
                    return Ok(LiteralValue::String(unescape(&raw)));
                }
                if self.input[self.pos..].starts_with("^^") {
                    self.bump();
                    self.bump();
                    let dt = if self.peek_char() == Some('<') {
                        self.parse_iriref()?
                    } else {
                        let w = self.parse_word();
                        self.expand_prefixed(&w)?
                    };
                    return Ok(coerce_literal(unescape(&raw), &dt));
                }
                return Ok(LiteralValue::String(unescape(&raw)));
            }
            self.bump();
        }
        Err(self.err("unterminated string"))
    }

    fn parse_word(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_whitespace()
                || matches!(
                    c,
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
                        | '&'
                        | '|'
                )
            {
                break;
            }
            self.bump();
        }
        self.input[start..self.pos].to_owned()
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

    fn err(&self, msg: impl Into<String>) -> OntolithError {
        OntolithError::parse_at(self.line, self.col, msg.into())
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
            function,
            output,
            input,
        } => {
            walk_physical(input, steps);
            match function {
                AggregateFunction::Count { variable: None } => {
                    steps.push(format!("aggregate_count:*->{output}"));
                }
                AggregateFunction::Count { variable: Some(v) } => {
                    steps.push(format!("aggregate_count:?{v}->?{output}"));
                }
            }
        }
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
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn coerce_literal(content: String, dt: &str) -> LiteralValue {
    match dt {
        "http://www.w3.org/2001/XMLSchema#integer"
        | "http://www.w3.org/2001/XMLSchema#int"
        | "http://www.w3.org/2001/XMLSchema#long" => content
            .parse()
            .map(LiteralValue::Integer)
            .unwrap_or(LiteralValue::String(content)),
        "http://www.w3.org/2001/XMLSchema#double"
        | "http://www.w3.org/2001/XMLSchema#float"
        | "http://www.w3.org/2001/XMLSchema#decimal" => content
            .parse()
            .map(LiteralValue::Decimal)
            .unwrap_or(LiteralValue::String(content)),
        "http://www.w3.org/2001/XMLSchema#boolean" => match content.as_str() {
            "true" | "1" => LiteralValue::Boolean(true),
            "false" | "0" => LiteralValue::Boolean(false),
            _ => LiteralValue::String(content),
        },
        _ => LiteralValue::String(content),
    }
}

fn is_integer(s: &str) -> bool {
    let t = s.trim_start_matches(['+', '-']);
    !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())
}

fn is_decimal(s: &str) -> bool {
    s.parse::<f64>().is_ok() && s.chars().any(|c| c == '.' || c == 'e' || c == 'E')
}
