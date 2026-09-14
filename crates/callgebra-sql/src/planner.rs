//! Validation of the CallSQL subset and resolution against the catalog.
//!
//! Produces [`Statement`]s whose [`LogicalPlan`]s reference columns by
//! position. Everything outside the subset is an [`SqlError`] with a hint.

use crate::error::SqlError;
use crate::expr::{AggregateFn, BinaryOp, Expr, Literal, UnaryOp};
use crate::plan::{JoinKind, LogicalPlan, SortKey};
use crate::scope::{CteEnv, Scope, ScopeItem};
use crate::similar::suggest;
use crate::statement::{Statement, StatementKind};
use crate::types::{is_bool_or_any, is_numeric_or_any, type_of};
use callgebra_core::{Catalog, DataType, Field, FunctionReturn, Schema, Value, Volatility};
use sqlparser::ast as sp;
use std::sync::Arc;

/// Planner over one catalog.
pub(crate) struct Planner<'c> {
    catalog: &'c Catalog,
}

/// Output of planning a relation: the plan and the columns it exposes.
struct Planned<'a> {
    plan: LogicalPlan,
    scope: Scope<'a>,
}

fn unsupported(construct: impl Into<String>, hint: impl Into<String>) -> SqlError {
    SqlError::Unsupported {
        construct: construct.into(),
        hint: hint.into(),
    }
}

fn type_err(message: impl Into<String>) -> SqlError {
    SqlError::Type {
        message: message.into(),
    }
}

fn ident_str(i: &sp::Ident) -> String {
    i.value.clone()
}

fn object_name_last(n: &sp::ObjectName) -> Result<String, SqlError> {
    match n.0.last() {
        Some(sp::ObjectNamePart::Identifier(i)) => Ok(ident_str(i)),
        _ => Err(unsupported(
            format!("name {n}"),
            "use a plain identifier".to_string(),
        )),
    }
}

fn alias_name(alias: &Option<sp::TableAlias>) -> Option<String> {
    alias.as_ref().map(|a| ident_str(&a.name))
}

fn schema_of(items: &[ScopeItem]) -> Arc<Schema> {
    Arc::new(Schema::new(
        items
            .iter()
            .map(|i| Field::new(i.name.clone(), i.data_type))
            .collect(),
    ))
}

fn items_from_schema(qualifier: Option<&str>, schema: &Schema) -> Vec<ScopeItem> {
    schema
        .fields
        .iter()
        .map(|f| ScopeItem {
            qualifier: qualifier.map(str::to_string),
            name: f.name.clone(),
            data_type: f.data_type,
        })
        .collect()
}

fn derived_name(e: &sp::Expr) -> String {
    match e {
        sp::Expr::Identifier(i) => ident_str(i),
        sp::Expr::CompoundIdentifier(parts) => parts.last().map(ident_str).unwrap_or_default(),
        sp::Expr::Function(f) => object_name_last(&f.name)
            .map(|n| n.to_ascii_lowercase())
            .unwrap_or_else(|_| "expr".into()),
        sp::Expr::Nested(inner) => derived_name(inner),
        other => format!("{other}"),
    }
}

fn aggregate_of(name: &str) -> Option<AggregateFn> {
    Some(match name {
        "count" => AggregateFn::Count,
        "sum" => AggregateFn::Sum,
        "min" => AggregateFn::Min,
        "max" => AggregateFn::Max,
        "avg" => AggregateFn::Avg,
        "string_agg" => AggregateFn::StringAgg,
        "bool_and" => AggregateFn::BoolAnd,
        "bool_or" => AggregateFn::BoolOr,
        _ => return None,
    })
}

fn contains_aggregate(e: &Expr) -> bool {
    match e {
        Expr::Aggregate { .. } => true,
        Expr::Column { .. } | Expr::Literal(_) | Expr::OuterColumn { .. } => false,
        Expr::Binary { left, right, .. } => contains_aggregate(left) || contains_aggregate(right),
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => contains_aggregate(operand),
        Expr::Function { args, .. } => args.iter().any(contains_aggregate),
        Expr::Case {
            branches,
            otherwise,
        } => {
            branches
                .iter()
                .any(|(c, r)| contains_aggregate(c) || contains_aggregate(r))
                || otherwise.as_deref().is_some_and(contains_aggregate)
        }
        Expr::InList { operand, list, .. } => {
            contains_aggregate(operand) || list.iter().any(contains_aggregate)
        }
        Expr::InSubquery { operand, .. } => contains_aggregate(operand),
        Expr::Exists { .. } | Expr::ScalarSubquery { .. } => false,
    }
}

/// Rewrite an expression over the pre-aggregation input into one over the
/// aggregate output (`groups ++ aggregates`).
fn lift(e: Expr, groups: &[Expr], aggs: &mut Vec<Expr>) -> Result<Expr, SqlError> {
    if let Some(pos) = groups.iter().position(|g| *g == e) {
        let name = match &e {
            Expr::Column { name, .. } => name.clone(),
            _ => format!("group{pos}"),
        };
        return Ok(Expr::Column { index: pos, name });
    }
    Ok(match e {
        Expr::Aggregate { .. } => {
            let idx = match aggs.iter().position(|a| *a == e) {
                Some(i) => i,
                None => {
                    aggs.push(e);
                    aggs.len() - 1
                }
            };
            Expr::Column {
                index: groups.len() + idx,
                name: format!("agg{idx}"),
            }
        }
        Expr::Column { name, .. } => {
            return Err(type_err(format!(
                "column {name} must appear in GROUP BY or be used in an aggregate function"
            )))
        }
        Expr::Literal(_) | Expr::OuterColumn { .. } => e,
        Expr::Binary { op, left, right } => Expr::Binary {
            op,
            left: Box::new(lift(*left, groups, aggs)?),
            right: Box::new(lift(*right, groups, aggs)?),
        },
        Expr::Unary { op, operand } => Expr::Unary {
            op,
            operand: Box::new(lift(*operand, groups, aggs)?),
        },
        Expr::Cast { operand, to } => Expr::Cast {
            operand: Box::new(lift(*operand, groups, aggs)?),
            to,
        },
        Expr::Function {
            name,
            args,
            data_type,
        } => Expr::Function {
            name,
            args: args
                .into_iter()
                .map(|a| lift(a, groups, aggs))
                .collect::<Result<_, _>>()?,
            data_type,
        },
        Expr::Case {
            branches,
            otherwise,
        } => Expr::Case {
            branches: branches
                .into_iter()
                .map(|(c, r)| Ok((lift(c, groups, aggs)?, lift(r, groups, aggs)?)))
                .collect::<Result<_, SqlError>>()?,
            otherwise: match otherwise {
                Some(o) => Some(Box::new(lift(*o, groups, aggs)?)),
                None => None,
            },
        },
        Expr::InList {
            operand,
            list,
            negated,
        } => Expr::InList {
            operand: Box::new(lift(*operand, groups, aggs)?),
            list: list
                .into_iter()
                .map(|a| lift(a, groups, aggs))
                .collect::<Result<_, _>>()?,
            negated,
        },
        Expr::InSubquery { .. } | Expr::Exists { .. } | Expr::ScalarSubquery { .. } => {
            return Err(unsupported(
                "subquery inside an aggregated SELECT list or HAVING",
                "compute the subquery in a CTE and join it",
            ))
        }
    })
}

impl<'c> Planner<'c> {
    pub fn new(catalog: &'c Catalog) -> Self {
        Self { catalog }
    }

    // ----------------------------------------------------------------- statements

    pub fn plan_statement(&self, stmt: &sp::Statement, sql: String) -> Result<Statement, SqlError> {
        let kind = match stmt {
            sp::Statement::Query(q) => StatementKind::Query {
                plan: self.plan_query(q, None, &CteEnv::default())?.plan,
            },
            sp::Statement::CreateTable(ct) => {
                let name = object_name_last(&ct.name)?;
                let Some(query) = &ct.query else {
                    return Err(unsupported(
                        "CREATE TABLE with a column list",
                        "use CREATE TABLE name AS SELECT ... (column DDL arrives later)",
                    ));
                };
                StatementKind::CreateTableAs {
                    name,
                    plan: self.plan_query(query, None, &CteEnv::default())?.plan,
                    if_not_exists: ct.if_not_exists,
                }
            }
            sp::Statement::Insert(ins) => self.plan_insert(ins)?,
            sp::Statement::Drop {
                object_type,
                if_exists,
                names,
                ..
            } => {
                if *object_type != sp::ObjectType::Table {
                    return Err(unsupported(
                        format!("DROP {object_type}"),
                        "only DROP TABLE is supported",
                    ));
                }
                if names.len() != 1 {
                    return Err(unsupported(
                        "DROP TABLE with several names",
                        "drop one table per statement",
                    ));
                }
                StatementKind::DropTable {
                    name: object_name_last(&names[0])?,
                    if_exists: *if_exists,
                }
            }
            sp::Statement::Explain {
                analyze, statement, ..
            } => StatementKind::Explain {
                inner: Box::new(self.plan_statement(statement, statement.to_string())?),
                analyze: *analyze,
            },
            sp::Statement::Set(sp::Set::SingleAssignment {
                variable, values, ..
            }) => {
                let key = variable
                    .0
                    .iter()
                    .map(|p| match p {
                        sp::ObjectNamePart::Identifier(i) => ident_str(i),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(".");
                let value = match values.first() {
                    Some(sp::Expr::Value(v)) => match &v.value {
                        sp::Value::SingleQuotedString(s) | sp::Value::DoubleQuotedString(s) => {
                            s.clone()
                        }
                        other => other.to_string(),
                    },
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                StatementKind::Set { key, value }
            }
            sp::Statement::Set(_) => {
                return Err(unsupported("this form of SET", "use SET key = value"))
            }
            sp::Statement::CreateFunction(_) => {
                return Err(unsupported(
                    "this CREATE FUNCTION form",
                    "CREATE FUNCTION name(x TEXT) RETURNS TYPE AS PROMPT '...' | AS SQL (...) | AS SHELL '...'",
                ))
            }
            sp::Statement::Call(_) => {
                return Err(unsupported(
                    "CALL inside a multi-statement text",
                    "submit CALL tool(args) [FROM query] on its own",
                ))
            }
            sp::Statement::Update(_) | sp::Statement::Delete(_) => {
                return Err(unsupported(
                    "UPDATE / DELETE",
                    "tables are append-only: CREATE TABLE ... AS SELECT a filtered copy instead",
                ))
            }
            other => {
                return Err(unsupported(
                    other.to_string().split_whitespace().take(2).collect::<Vec<_>>().join(" "),
                    "supported statements: SELECT, WITH, CREATE TABLE AS, INSERT, DROP TABLE, EXPLAIN, SET, FINAL",
                ))
            }
        };
        Ok(Statement { sql, kind })
    }

    fn plan_insert(&self, ins: &sp::Insert) -> Result<StatementKind, SqlError> {
        let table = match &ins.table {
            sp::TableObject::TableName(n) => object_name_last(n)?,
            _ => {
                return Err(unsupported(
                    "INSERT INTO a function",
                    "insert into a named table",
                ))
            }
        };
        let def = self
            .catalog
            .table(&table)
            .ok_or_else(|| SqlError::Unresolved {
                what: "table",
                name: table.clone(),
                hint: suggest(&table, self.catalog.tables().map(|t| t.name.as_str())),
            })?;
        let Some(source) = &ins.source else {
            return Err(unsupported(
                "INSERT without VALUES or SELECT",
                "INSERT INTO t VALUES (...) or INSERT INTO t SELECT ...",
            ));
        };
        let planned = self.plan_query(source, None, &CteEnv::default())?;
        let target_cols: Vec<String> = if ins.columns.is_empty() {
            def.schema.fields.iter().map(|f| f.name.clone()).collect()
        } else {
            ins.columns
                .iter()
                .map(object_name_last)
                .collect::<Result<_, _>>()?
        };
        if planned.scope.items.len() != target_cols.len() {
            return Err(type_err(format!(
                "INSERT supplies {} values but {} expects {} columns ({}); list the target columns or match the arity",
                planned.scope.items.len(),
                table,
                target_cols.len(),
                target_cols.join(", ")
            )));
        }
        // Reorder into table column order, NULL for columns not listed.
        let mut exprs = Vec::with_capacity(def.schema.len());
        for f in &def.schema.fields {
            let pos = target_cols
                .iter()
                .position(|c| c.eq_ignore_ascii_case(&f.name));
            exprs.push(match pos {
                Some(p) => Expr::Column {
                    index: p,
                    name: f.name.clone(),
                },
                None => Expr::Literal(Literal(Value::Null)),
            });
        }
        for c in &target_cols {
            if def.schema.index_of(c).is_none() {
                return Err(SqlError::Unresolved {
                    what: "column",
                    name: c.clone(),
                    hint: suggest(c, def.schema.fields.iter().map(|f| f.name.as_str())),
                });
            }
        }
        let plan = LogicalPlan::Project {
            input: Box::new(planned.plan),
            exprs,
            schema: Arc::new(def.schema.clone()),
        };
        Ok(StatementKind::Insert { table, plan })
    }

    // ---------------------------------------------------------------- queries

    fn plan_query<'a>(
        &self,
        q: &sp::Query,
        outer: Option<&'a Scope<'a>>,
        ctes: &CteEnv,
    ) -> Result<Planned<'a>, SqlError> {
        if q.fetch.is_some() || !q.locks.is_empty() || q.for_clause.is_some() {
            return Err(unsupported("FETCH / FOR clauses", "use LIMIT and OFFSET"));
        }
        if !q.pipe_operators.is_empty() {
            return Err(unsupported("pipe operators", "write a nested SELECT"));
        }
        let mut env = ctes.clone();
        let mut bound: Vec<(String, LogicalPlan)> = vec![];
        let mut recursive: Option<(String, LogicalPlan, LogicalPlan, bool, Arc<Schema>)> = None;
        if let Some(with) = &q.with {
            for cte in &with.cte_tables {
                let name = ident_str(&cte.alias.name);
                if with.recursive && Self::cte_is_self_referential(&cte.query, &name) {
                    if recursive.is_some() {
                        return Err(unsupported(
                            "more than one recursive CTE",
                            "one WITH RECURSIVE term per statement; chain statements with CREATE TABLE AS",
                        ));
                    }
                    let (base, rec, all, schema) =
                        self.plan_recursive_cte(cte, &name, outer, &env)?;
                    env = env.with(&name, schema.clone());
                    recursive = Some((name, base, rec, all, schema));
                } else {
                    let planned = self.plan_query(&cte.query, outer, &env)?;
                    let plan =
                        Self::apply_column_aliases(planned.plan, &planned.scope, &cte.alias)?;
                    env = env.with(&name, plan.schema());
                    bound.push((name, plan));
                }
            }
        }
        let body = self.plan_set_expr(
            &q.body,
            q.order_by.as_ref(),
            q.limit_clause.as_ref(),
            outer,
            &env,
        )?;
        let mut plan = body.plan;
        if let Some((name, base, rec, all, _)) = recursive {
            plan = LogicalPlan::Recursive {
                name,
                base: Box::new(base),
                recursive: Box::new(rec),
                all,
                max_rounds: None,
                body: Box::new(plan),
            };
        }
        if !bound.is_empty() {
            plan = LogicalPlan::With {
                ctes: bound,
                body: Box::new(plan),
            };
        }
        Ok(Planned {
            plan,
            scope: body.scope,
        })
    }

    fn cte_is_self_referential(q: &sp::Query, name: &str) -> bool {
        // A recursive CTE is `base UNION [ALL] recursive`; detect the name in the right side.
        let text = q.to_string().to_ascii_lowercase();
        let lower = name.to_ascii_lowercase();
        matches!(
            &*q.body,
            sp::SetExpr::SetOperation {
                op: sp::SetOperator::Union,
                ..
            }
        ) && text
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|w| w == lower)
    }

    fn apply_column_aliases(
        plan: LogicalPlan,
        scope: &Scope<'_>,
        alias: &sp::TableAlias,
    ) -> Result<LogicalPlan, SqlError> {
        if alias.columns.is_empty() {
            return Ok(plan);
        }
        if alias.columns.len() != scope.items.len() {
            return Err(type_err(format!(
                "{} lists {} column names but the query has {} columns",
                ident_str(&alias.name),
                alias.columns.len(),
                scope.items.len()
            )));
        }
        let exprs: Vec<Expr> = scope
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| Expr::Column {
                index: i,
                name: it.name.clone(),
            })
            .collect();
        let schema = Arc::new(Schema::new(
            alias
                .columns
                .iter()
                .zip(&scope.items)
                .map(|(c, it)| Field::new(ident_str(&c.name), it.data_type))
                .collect(),
        ));
        Ok(LogicalPlan::Project {
            input: Box::new(plan),
            exprs,
            schema,
        })
    }

    fn plan_recursive_cte<'a>(
        &self,
        cte: &sp::Cte,
        name: &str,
        outer: Option<&'a Scope<'a>>,
        env: &CteEnv,
    ) -> Result<(LogicalPlan, LogicalPlan, bool, Arc<Schema>), SqlError> {
        let sp::SetExpr::SetOperation {
            left,
            op: sp::SetOperator::Union,
            set_quantifier,
            right,
        } = &*cte.query.body
        else {
            return Err(unsupported(
                "recursive CTE without UNION",
                "WITH RECURSIVE name AS (base_query UNION [ALL] recursive_query)",
            ));
        };
        // A trailing ORDER BY / LIMIT on a recursive CTE applies to each
        // round's recursive term: keep the best `k` new rows per round, a
        // beam search. (Ordering the final result belongs in the body.)
        if cte.query.order_by.is_some() && cte.query.limit_clause.is_none() {
            return Err(unsupported(
                "ORDER BY without LIMIT inside a recursive CTE",
                "ORDER BY score LIMIT k here keeps the top k rows of each round (beam search); order the final result in the body that uses the CTE",
            ));
        }
        let all = matches!(
            set_quantifier,
            sp::SetQuantifier::All | sp::SetQuantifier::AllByName
        );
        let base = self.plan_set_expr(left, None, None, outer, env)?;
        let base_plan = Self::apply_column_aliases(base.plan, &base.scope, &cte.alias)?;
        let schema = base_plan.schema();
        let env2 = env.with(name, schema.clone());
        let rec = self.plan_set_expr(
            right,
            cte.query.order_by.as_ref(),
            cte.query.limit_clause.as_ref(),
            outer,
            &env2,
        )?;
        if rec.scope.items.len() != schema.len() {
            return Err(type_err(format!(
                "recursive term of {name} has {} columns, base has {}",
                rec.scope.items.len(),
                schema.len()
            )));
        }
        Ok((base_plan, rec.plan, all, schema))
    }

    fn plan_set_expr<'a>(
        &self,
        body: &sp::SetExpr,
        order_by: Option<&sp::OrderBy>,
        limit: Option<&sp::LimitClause>,
        outer: Option<&'a Scope<'a>>,
        env: &CteEnv,
    ) -> Result<Planned<'a>, SqlError> {
        match body {
            sp::SetExpr::Select(select) => self.plan_select(select, order_by, limit, outer, env),
            sp::SetExpr::Query(q) => {
                let inner = self.plan_query(q, outer, env)?;
                self.apply_order_limit_to_output(inner, order_by, limit, outer)
            }
            sp::SetExpr::SetOperation {
                left,
                op,
                set_quantifier,
                right,
            } => {
                if *op != sp::SetOperator::Union {
                    return Err(unsupported(
                        format!("{op}"),
                        "use UNION [ALL]; express EXCEPT with NOT EXISTS and INTERSECT with EXISTS",
                    ));
                }
                let all = matches!(
                    set_quantifier,
                    sp::SetQuantifier::All | sp::SetQuantifier::AllByName
                );
                let l = self.plan_set_expr(left, None, None, outer, env)?;
                let r = self.plan_set_expr(right, None, None, outer, env)?;
                if l.scope.items.len() != r.scope.items.len() {
                    return Err(type_err(format!(
                        "UNION sides have {} and {} columns",
                        l.scope.items.len(),
                        r.scope.items.len()
                    )));
                }
                let mut inputs = vec![];
                for side in [l.plan, r.plan] {
                    match side {
                        LogicalPlan::Union {
                            inputs: inner,
                            all: inner_all,
                        } if inner_all == all => inputs.extend(inner),
                        other => inputs.push(other),
                    }
                }
                let scope = Scope {
                    items: l
                        .scope
                        .items
                        .into_iter()
                        .map(|mut it| {
                            it.qualifier = None;
                            it
                        })
                        .collect(),
                    parent: outer,
                };
                let planned = Planned {
                    plan: LogicalPlan::Union { inputs, all },
                    scope,
                };
                self.apply_order_limit_to_output(planned, order_by, limit, outer)
            }
            sp::SetExpr::Values(values) => {
                let planned = self.plan_values(values, outer)?;
                self.apply_order_limit_to_output(planned, order_by, limit, outer)
            }
            other => Err(unsupported(
                other
                    .to_string()
                    .split_whitespace()
                    .next()
                    .unwrap_or("statement")
                    .to_string(),
                "only SELECT, VALUES and UNION are allowed in a query body",
            )),
        }
    }

    fn plan_values<'a>(
        &self,
        values: &sp::Values,
        outer: Option<&'a Scope<'a>>,
    ) -> Result<Planned<'a>, SqlError> {
        let empty = Scope::empty(outer);
        let mut rows: Vec<Vec<Expr>> = vec![];
        for row in &values.rows {
            let exprs = row
                .content
                .iter()
                .map(|e| self.resolve_expr(e, &empty, false))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(first) = rows.first() {
                if first.len() != exprs.len() {
                    return Err(type_err("VALUES rows have different lengths"));
                }
            }
            rows.push(exprs);
        }
        let first = rows.first().cloned().unwrap_or_default();
        let empty_schema = Schema::empty();
        let items: Vec<ScopeItem> = first
            .iter()
            .enumerate()
            .map(|(i, e)| ScopeItem {
                qualifier: None,
                name: format!("col{i}"),
                data_type: type_of(e, &empty_schema),
            })
            .collect();
        let schema = schema_of(&items);
        Ok(Planned {
            plan: LogicalPlan::Values { rows, schema },
            scope: Scope {
                items,
                parent: outer,
            },
        })
    }

    /// ORDER BY / LIMIT on a relation whose only addressable columns are its
    /// outputs (set operations, VALUES, nested queries).
    fn apply_order_limit_to_output<'a>(
        &self,
        planned: Planned<'a>,
        order_by: Option<&sp::OrderBy>,
        limit: Option<&sp::LimitClause>,
        outer: Option<&'a Scope<'a>>,
    ) -> Result<Planned<'a>, SqlError> {
        let mut plan = planned.plan;
        let scope = Scope {
            items: planned.scope.items.clone(),
            parent: outer,
        };
        if let Some(ob) = order_by {
            let keys = self.resolve_order_keys(ob, &scope, None, &[])?;
            let keys = keys.into_iter().map(|(k, _)| k).collect::<Vec<_>>();
            plan = LogicalPlan::Sort {
                input: Box::new(plan),
                keys,
            };
        }
        plan = Self::apply_limit(plan, limit)?;
        Ok(Planned { plan, scope })
    }

    fn apply_limit(
        plan: LogicalPlan,
        limit: Option<&sp::LimitClause>,
    ) -> Result<LogicalPlan, SqlError> {
        let Some(l) = limit else { return Ok(plan) };
        let (lim, off) = match l {
            sp::LimitClause::LimitOffset {
                limit,
                offset,
                limit_by,
            } => {
                if !limit_by.is_empty() {
                    return Err(unsupported("LIMIT BY", "use LIMIT n OFFSET m"));
                }
                (
                    limit.as_ref().map(Self::literal_usize).transpose()?,
                    offset
                        .as_ref()
                        .map(|o| Self::literal_usize(&o.value))
                        .transpose()?
                        .unwrap_or(0),
                )
            }
            sp::LimitClause::OffsetCommaLimit { offset, limit } => (
                Some(Self::literal_usize(limit)?),
                Self::literal_usize(offset)?,
            ),
        };
        Ok(LogicalPlan::Limit {
            input: Box::new(plan),
            offset: off,
            limit: lim,
        })
    }

    fn literal_usize(e: &sp::Expr) -> Result<usize, SqlError> {
        match e {
            sp::Expr::Value(v) => match &v.value {
                sp::Value::Number(n, _) => n.parse::<usize>().map_err(|_| {
                    type_err(format!(
                        "LIMIT/OFFSET must be a non-negative integer, got {n}"
                    ))
                }),
                other => Err(type_err(format!(
                    "LIMIT/OFFSET must be an integer literal, got {other}"
                ))),
            },
            other => Err(unsupported(
                format!("LIMIT/OFFSET expression {other}"),
                "use an integer literal",
            )),
        }
    }

    // ----------------------------------------------------------------- SELECT

    fn plan_select<'a>(
        &self,
        select: &sp::Select,
        order_by: Option<&sp::OrderBy>,
        limit: Option<&sp::LimitClause>,
        outer: Option<&'a Scope<'a>>,
        env: &CteEnv,
    ) -> Result<Planned<'a>, SqlError> {
        if select.top.is_some() || select.into.is_some() || !select.lateral_views.is_empty() {
            return Err(unsupported(
                "TOP / INTO / LATERAL VIEW",
                "use LIMIT, CREATE TABLE AS, or LATERAL f(...)",
            ));
        }
        if select.qualify.is_some()
            || !select.named_window.is_empty()
            || !select.cluster_by.is_empty()
            || !select.distribute_by.is_empty()
            || !select.sort_by.is_empty()
            || select.prewhere.is_some()
            || !select.connect_by.is_empty()
        {
            return Err(unsupported(
                "QUALIFY / WINDOW / CLUSTER BY / SORT BY / PREWHERE / CONNECT BY",
                "not part of CallSQL",
            ));
        }
        if select.exclude.is_some() {
            return Err(unsupported("SELECT * EXCLUDE", "list the columns you want"));
        }
        if matches!(select.distinct, Some(sp::Distinct::On(_))) {
            return Err(unsupported(
                "DISTINCT ON",
                "use GROUP BY or a window-free rewrite",
            ));
        }

        // FROM
        let from = self.plan_from(&select.from, outer, env)?;
        let mut plan = from.plan;
        let from_scope = Scope {
            items: from.scope.items,
            parent: outer,
        };

        // WHERE
        if let Some(sel) = &select.selection {
            let pred = self.resolve_expr(sel, &from_scope, false)?;
            self.require_bool(&pred, &from_scope, "WHERE")?;
            plan = LogicalPlan::Filter {
                input: Box::new(plan),
                predicate: pred,
            };
        }

        // Projection expressions over the FROM scope.
        let mut proj: Vec<(Expr, String)> = vec![];
        for item in &select.projection {
            match item {
                sp::SelectItem::Wildcard(_) => {
                    for (i, it) in from_scope.items.iter().enumerate() {
                        proj.push((
                            Expr::Column {
                                index: i,
                                name: it.name.clone(),
                            },
                            it.name.clone(),
                        ));
                    }
                }
                sp::SelectItem::QualifiedWildcard(kind, _) => {
                    let q = match kind {
                        sp::SelectItemQualifiedWildcardKind::ObjectName(n) => object_name_last(n)?,
                        sp::SelectItemQualifiedWildcardKind::Expr(e) => {
                            return Err(unsupported(format!("{e}.*"), "use alias.*"))
                        }
                    };
                    let cols = from_scope.qualified(&q);
                    if cols.is_empty() {
                        return Err(SqlError::Unresolved {
                            what: "table",
                            name: q,
                            hint: Some("no relation with that alias in FROM".into()),
                        });
                    }
                    for (i, it) in cols {
                        proj.push((
                            Expr::Column {
                                index: i,
                                name: it.name.clone(),
                            },
                            it.name.clone(),
                        ));
                    }
                }
                sp::SelectItem::UnnamedExpr(e) => {
                    proj.push((self.resolve_expr(e, &from_scope, true)?, derived_name(e)));
                }
                sp::SelectItem::ExprWithAlias { expr, alias } => {
                    proj.push((
                        self.resolve_expr(expr, &from_scope, true)?,
                        ident_str(alias),
                    ));
                }
                sp::SelectItem::ExprWithAliases { .. } => {
                    return Err(unsupported("multiple aliases", "one alias per expression"))
                }
            }
        }

        // GROUP BY / aggregates
        let group_exprs: Vec<sp::Expr> = match &select.group_by {
            sp::GroupByExpr::Expressions(exprs, modifiers) => {
                if !modifiers.is_empty() {
                    return Err(unsupported(
                        "GROUP BY modifiers (ROLLUP/CUBE)",
                        "plain GROUP BY",
                    ));
                }
                exprs.clone()
            }
            sp::GroupByExpr::All(_) => {
                return Err(unsupported("GROUP BY ALL", "list the grouping columns"))
            }
        };
        let having = select
            .having
            .as_ref()
            .map(|h| self.resolve_expr(h, &from_scope, true))
            .transpose()?;
        let is_aggregate = !group_exprs.is_empty()
            || proj.iter().any(|(e, _)| contains_aggregate(e))
            || having.as_ref().is_some_and(contains_aggregate);

        // The scope that ORDER BY expressions resolve against, and the plan they apply over.
        let pre_scope: Scope<'a>;
        let pre_exprs: Vec<Expr>;
        if is_aggregate {
            let mut groups: Vec<Expr> = vec![];
            for g in &group_exprs {
                let ge = match g {
                    sp::Expr::Value(v) if matches!(v.value, sp::Value::Number(_, _)) => {
                        let ord = Self::literal_usize(g)?;
                        let (e, _) = proj.get(ord.wrapping_sub(1)).ok_or_else(|| {
                            type_err(format!(
                                "GROUP BY {ord} is out of range for {} select items",
                                proj.len()
                            ))
                        })?;
                        e.clone()
                    }
                    sp::Expr::Identifier(id)
                        if from_scope
                            .items
                            .iter()
                            .all(|it| !it.name.eq_ignore_ascii_case(&id.value))
                            && proj
                                .iter()
                                .filter(|(_, n)| n.eq_ignore_ascii_case(&id.value))
                                .count()
                                == 1 =>
                    {
                        proj.iter()
                            .find(|(_, n)| n.eq_ignore_ascii_case(&id.value))
                            .map(|(e, _)| e.clone())
                            .unwrap()
                    }
                    other => self.resolve_expr(other, &from_scope, false)?,
                };
                if contains_aggregate(&ge) {
                    return Err(type_err("aggregate functions are not allowed in GROUP BY"));
                }
                groups.push(ge);
            }
            let mut aggs: Vec<Expr> = vec![];
            let lifted_proj: Vec<(Expr, String)> = proj
                .iter()
                .map(|(e, n)| Ok((lift(e.clone(), &groups, &mut aggs)?, n.clone())))
                .collect::<Result<_, SqlError>>()?;
            let lifted_having = having.map(|h| lift(h, &groups, &mut aggs)).transpose()?;
            let input_schema = from_scope.schema();
            let mut agg_items: Vec<ScopeItem> = groups
                .iter()
                .enumerate()
                .map(|(i, g)| ScopeItem {
                    qualifier: None,
                    name: match g {
                        Expr::Column { name, .. } => name.clone(),
                        _ => format!("group{i}"),
                    },
                    data_type: type_of(g, &input_schema),
                })
                .collect();
            agg_items.extend(aggs.iter().enumerate().map(|(i, a)| ScopeItem {
                qualifier: None,
                name: format!("agg{i}"),
                data_type: type_of(a, &input_schema),
            }));
            plan = LogicalPlan::Aggregate {
                input: Box::new(plan),
                group_by: groups,
                aggregates: aggs,
                schema: schema_of(&agg_items),
            };
            if let Some(h) = lifted_having {
                plan = LogicalPlan::Filter {
                    input: Box::new(plan),
                    predicate: h,
                };
            }
            pre_scope = Scope {
                items: agg_items,
                parent: outer,
            };
            pre_exprs = lifted_proj.iter().map(|(e, _)| e.clone()).collect();
            proj = lifted_proj;
        } else {
            if having.is_some() {
                return Err(type_err("HAVING requires GROUP BY or an aggregate"));
            }
            pre_scope = from_scope;
            pre_exprs = proj.iter().map(|(e, _)| e.clone()).collect();
        }

        // Output scope of the projection.
        let pre_schema = pre_scope.schema();
        let out_items: Vec<ScopeItem> = proj
            .iter()
            .map(|(e, n)| ScopeItem {
                qualifier: None,
                name: n.clone(),
                data_type: type_of(e, &pre_schema),
            })
            .collect();
        let out_scope = Scope {
            items: out_items.clone(),
            parent: outer,
        };
        let distinct = matches!(select.distinct, Some(sp::Distinct::Distinct));

        // ORDER BY: keys over the output where possible, else extra projected columns.
        let mut extras: Vec<Expr> = vec![];
        let mut keys: Vec<SortKey> = vec![];
        if let Some(ob) = order_by {
            let resolved = self.resolve_order_keys(ob, &out_scope, Some(&pre_scope), &pre_exprs)?;
            for (key, extra) in resolved {
                match extra {
                    None => keys.push(key),
                    Some(extra_expr) => {
                        if distinct {
                            return Err(unsupported(
                                "ORDER BY a column that is not in the SELECT DISTINCT list",
                                "add the column to the select list",
                            ));
                        }
                        let idx = pre_exprs.len() + extras.len();
                        extras.push(extra_expr);
                        keys.push(SortKey {
                            expr: Expr::Column {
                                index: idx,
                                name: format!("order{idx}"),
                            },
                            asc: key.asc,
                            nulls_first: key.nulls_first,
                        });
                    }
                }
            }
        }

        let mut project_exprs = pre_exprs.clone();
        project_exprs.extend(extras.iter().cloned());
        let mut project_items = out_items.clone();
        project_items.extend(extras.iter().enumerate().map(|(i, e)| ScopeItem {
            qualifier: None,
            name: format!("order{}", pre_exprs.len() + i),
            data_type: type_of(e, &pre_schema),
        }));
        plan = LogicalPlan::Project {
            input: Box::new(plan),
            exprs: project_exprs,
            schema: schema_of(&project_items),
        };
        if distinct {
            plan = LogicalPlan::Distinct {
                input: Box::new(plan),
            };
        }
        if !keys.is_empty() {
            plan = LogicalPlan::Sort {
                input: Box::new(plan),
                keys,
            };
        }
        if !extras.is_empty() {
            plan = LogicalPlan::Project {
                input: Box::new(plan),
                exprs: out_items
                    .iter()
                    .enumerate()
                    .map(|(i, it)| Expr::Column {
                        index: i,
                        name: it.name.clone(),
                    })
                    .collect(),
                schema: schema_of(&out_items),
            };
        }
        plan = Self::apply_limit(plan, limit)?;
        Ok(Planned {
            plan,
            scope: out_scope,
        })
    }

    /// Resolve ORDER BY keys. Returns, per key, the sort key over the output
    /// scope and, when the key had to be computed from the pre-projection
    /// scope, the extra expression to project.
    fn resolve_order_keys(
        &self,
        ob: &sp::OrderBy,
        out_scope: &Scope<'_>,
        pre_scope: Option<&Scope<'_>>,
        pre_exprs: &[Expr],
    ) -> Result<Vec<(SortKey, Option<Expr>)>, SqlError> {
        let exprs = match &ob.kind {
            sp::OrderByKind::Expressions(e) => e,
            sp::OrderByKind::All(_) => {
                return Err(unsupported("ORDER BY ALL", "list the sort keys"))
            }
        };
        if ob.interpolate.is_some() {
            return Err(unsupported("INTERPOLATE", "not part of CallSQL"));
        }
        let mut out = vec![];
        for obe in exprs {
            if obe.with_fill.is_some() {
                return Err(unsupported("WITH FILL", "not part of CallSQL"));
            }
            let asc = obe.options.asc.unwrap_or(true);
            // DuckDB/Postgres default: NULLS LAST for ASC, NULLS FIRST for DESC.
            let nulls_first = obe.options.nulls_first.unwrap_or(!asc);
            let mk = |expr: Expr| SortKey {
                expr,
                asc,
                nulls_first,
            };
            // Ordinal
            if let sp::Expr::Value(v) = &obe.expr {
                if matches!(v.value, sp::Value::Number(_, _)) {
                    let ord = Self::literal_usize(&obe.expr)?;
                    let it = out_scope.items.get(ord.wrapping_sub(1)).ok_or_else(|| {
                        type_err(format!(
                            "ORDER BY {ord} is out of range for {} output columns",
                            out_scope.items.len()
                        ))
                    })?;
                    out.push((
                        mk(Expr::Column {
                            index: ord - 1,
                            name: it.name.clone(),
                        }),
                        None,
                    ));
                    continue;
                }
            }
            // Output alias
            if let sp::Expr::Identifier(id) = &obe.expr {
                let hits: Vec<usize> = out_scope
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, it)| it.name.eq_ignore_ascii_case(&id.value))
                    .map(|(i, _)| i)
                    .collect();
                if hits.len() == 1 {
                    out.push((
                        mk(Expr::Column {
                            index: hits[0],
                            name: id.value.clone(),
                        }),
                        None,
                    ));
                    continue;
                }
            }
            // Expression over the pre-projection scope
            let Some(pre) = pre_scope else {
                let e = self.resolve_expr(&obe.expr, out_scope, false)?;
                out.push((mk(e), None));
                continue;
            };
            let e = self.resolve_expr(&obe.expr, pre, false)?;
            if let Some(pos) = pre_exprs.iter().position(|p| *p == e) {
                out.push((
                    mk(Expr::Column {
                        index: pos,
                        name: out_scope.items[pos].name.clone(),
                    }),
                    None,
                ));
            } else {
                out.push((mk(Expr::Literal(Literal(Value::Null))), Some(e)));
            }
        }
        Ok(out)
    }

    // ------------------------------------------------------------------- FROM

    fn plan_from<'a>(
        &self,
        from: &[sp::TableWithJoins],
        outer: Option<&'a Scope<'a>>,
        env: &CteEnv,
    ) -> Result<Planned<'a>, SqlError> {
        if from.is_empty() {
            return Ok(Planned {
                plan: LogicalPlan::Values {
                    rows: vec![vec![]],
                    schema: Arc::new(Schema::empty()),
                },
                scope: Scope::empty(outer),
            });
        }
        let mut acc: Option<Planned<'a>> = None;
        for twj in from {
            let mut cur = self.plan_table_factor(&twj.relation, None, outer, env)?;
            for join in &twj.joins {
                cur = self.plan_join(cur, join, outer, env)?;
            }
            acc = Some(match acc {
                None => cur,
                Some(left) => Self::cross(left, cur, outer),
            });
        }
        Ok(acc.expect("non-empty FROM"))
    }

    fn cross<'a>(
        left: Planned<'a>,
        right: Planned<'a>,
        outer: Option<&'a Scope<'a>>,
    ) -> Planned<'a> {
        let scope = Scope::concat(&left.scope, &right.scope, outer);
        Planned {
            plan: LogicalPlan::Join {
                left: Box::new(left.plan),
                right: Box::new(right.plan),
                kind: JoinKind::Cross,
                on: None,
                schema: scope.schema(),
            },
            scope,
        }
    }

    fn plan_join<'a>(
        &self,
        left: Planned<'a>,
        join: &sp::Join,
        outer: Option<&'a Scope<'a>>,
        env: &CteEnv,
    ) -> Result<Planned<'a>, SqlError> {
        if join.global {
            return Err(unsupported("GLOBAL JOIN", "plain JOIN"));
        }
        let (kind, constraint) = match &join.join_operator {
            sp::JoinOperator::Join(c) | sp::JoinOperator::Inner(c) => (JoinKind::Inner, Some(c)),
            sp::JoinOperator::Left(c) | sp::JoinOperator::LeftOuter(c) => (JoinKind::Left, Some(c)),
            sp::JoinOperator::CrossJoin(c) => (JoinKind::Cross, Some(c)),
            sp::JoinOperator::Right(_) | sp::JoinOperator::RightOuter(_) => {
                return Err(unsupported(
                    "RIGHT JOIN",
                    "swap the sides and use LEFT JOIN",
                ))
            }
            sp::JoinOperator::FullOuter(_) => {
                return Err(unsupported(
                    "FULL OUTER JOIN",
                    "UNION of a LEFT JOIN and the NOT EXISTS complement",
                ))
            }
            sp::JoinOperator::Semi(_)
            | sp::JoinOperator::LeftSemi(_)
            | sp::JoinOperator::Anti(_)
            | sp::JoinOperator::LeftAnti(_) => {
                return Err(unsupported(
                    "explicit SEMI/ANTI JOIN",
                    "write EXISTS / NOT EXISTS; the planner derives semi-joins",
                ))
            }
            other => {
                return Err(unsupported(
                    format!("{other:?}")
                        .split(['(', ' '])
                        .next()
                        .unwrap_or("join")
                        .to_string(),
                    "INNER, LEFT or CROSS JOIN",
                ))
            }
        };
        // LATERAL table function on the right sees the left's columns.
        let right = self.plan_table_factor(&join.relation, Some(&left), outer, env)?;
        if let LogicalPlan::TableFunction { input: Some(_), .. } = &right.plan {
            // The lateral function already consumed `left` as its input.
            if !matches!(kind, JoinKind::Cross | JoinKind::Inner) {
                return Err(unsupported(
                    "LEFT JOIN LATERAL",
                    "CROSS JOIN LATERAL f(...)",
                ));
            }
            if let Some(sp::JoinConstraint::On(_)) = constraint {
                return Err(unsupported(
                    "ON with LATERAL function",
                    "CROSS JOIN LATERAL f(...) and filter in WHERE",
                ));
            }
            return Ok(right);
        }
        let scope = Scope::concat(&left.scope, &right.scope, outer);
        let on = match (kind, constraint) {
            (JoinKind::Cross, Some(sp::JoinConstraint::None)) | (JoinKind::Cross, None) => None,
            (JoinKind::Cross, Some(_)) => {
                return Err(unsupported(
                    "CROSS JOIN with ON",
                    "drop ON or use JOIN ... ON",
                ))
            }
            (_, Some(sp::JoinConstraint::On(e))) => {
                let pred = self.resolve_expr(e, &scope, false)?;
                self.require_bool(&pred, &scope, "ON")?;
                Some(pred)
            }
            (_, Some(sp::JoinConstraint::Using(_))) => {
                return Err(unsupported("JOIN USING", "JOIN ... ON a.x = b.x"))
            }
            (_, Some(sp::JoinConstraint::Natural)) => {
                return Err(unsupported(
                    "NATURAL JOIN",
                    "JOIN ... ON with explicit columns",
                ))
            }
            (_, Some(sp::JoinConstraint::None)) | (_, None) => {
                return Err(unsupported(
                    "JOIN without ON",
                    "JOIN ... ON condition, or CROSS JOIN",
                ))
            }
        };
        Ok(Planned {
            plan: LogicalPlan::Join {
                left: Box::new(left.plan),
                right: Box::new(right.plan),
                kind,
                on,
                schema: scope.schema(),
            },
            scope,
        })
    }

    fn plan_table_factor<'a>(
        &self,
        tf: &sp::TableFactor,
        lateral_left: Option<&Planned<'a>>,
        outer: Option<&'a Scope<'a>>,
        env: &CteEnv,
    ) -> Result<Planned<'a>, SqlError> {
        match tf {
            sp::TableFactor::Table {
                name, alias, args, ..
            } => {
                let tname = object_name_last(name)?;
                if let Some(a) = args {
                    // FROM f(args): a plain table function.
                    return self.plan_table_function(&tname, &a.args, alias, None, outer);
                }
                let qualifier = alias_name(alias).unwrap_or_else(|| tname.clone());
                if let Some(schema) = env.get(&tname) {
                    let plan = LogicalPlan::CteRef {
                        name: tname.to_ascii_lowercase(),
                        schema: schema.clone(),
                    };
                    let items = items_from_schema(Some(&qualifier), &schema);
                    return Ok(Planned {
                        plan,
                        scope: Scope {
                            items,
                            parent: outer,
                        },
                    });
                }
                let def = self
                    .catalog
                    .table(&tname)
                    .ok_or_else(|| SqlError::Unresolved {
                        what: "table",
                        name: tname.clone(),
                        hint: suggest(
                            &tname,
                            self.catalog
                                .tables()
                                .map(|t| t.name.as_str())
                                .chain(env.names()),
                        ),
                    })?;
                let schema = Arc::new(def.schema.clone());
                let plan = LogicalPlan::Scan {
                    table: def.name.clone(),
                    schema: schema.clone(),
                };
                let items = items_from_schema(Some(&qualifier), &schema);
                Ok(Planned {
                    plan,
                    scope: Scope {
                        items,
                        parent: outer,
                    },
                })
            }
            sp::TableFactor::Derived {
                lateral,
                subquery,
                alias,
                ..
            } => {
                if *lateral {
                    return Err(unsupported("LATERAL subquery", "CROSS JOIN LATERAL table_function(...) or a correlated subquery in the SELECT list"));
                }
                let inner = self.plan_query(subquery, outer, env)?;
                let qualifier = alias_name(alias).unwrap_or_else(|| "subquery".to_string());
                let plan = match alias {
                    Some(a) => Self::apply_column_aliases(inner.plan, &inner.scope, a)?,
                    None => inner.plan,
                };
                let schema = plan.schema();
                let items = items_from_schema(Some(&qualifier), &schema);
                Ok(Planned {
                    plan,
                    scope: Scope {
                        items,
                        parent: outer,
                    },
                })
            }
            sp::TableFactor::Function {
                lateral,
                name,
                args,
                alias,
                ..
            } => {
                let fname = object_name_last(name)?;
                if *lateral {
                    let Some(left) = lateral_left else {
                        return Err(unsupported(
                            "LATERAL as the first FROM item",
                            "put the relation it depends on first",
                        ));
                    };
                    self.plan_table_function(&fname, args, alias, Some(left), outer)
                } else {
                    self.plan_table_function(&fname, args, alias, None, outer)
                }
            }
            sp::TableFactor::NestedJoin {
                table_with_joins,
                alias,
            } => {
                if alias.is_some() {
                    return Err(unsupported(
                        "alias on a parenthesised join",
                        "alias the individual tables",
                    ));
                }
                let mut cur =
                    self.plan_table_factor(&table_with_joins.relation, None, outer, env)?;
                for j in &table_with_joins.joins {
                    cur = self.plan_join(cur, j, outer, env)?;
                }
                Ok(cur)
            }
            sp::TableFactor::UNNEST { .. } => Err(unsupported(
                "UNNEST",
                "a table function such as chunks(...) or generate_series(...)",
            )),
            other => Err(unsupported(
                other
                    .to_string()
                    .split_whitespace()
                    .next()
                    .unwrap_or("relation")
                    .to_string(),
                "tables, subqueries, table functions and joins",
            )),
        }
    }

    fn plan_table_function<'a>(
        &self,
        fname: &str,
        args: &[sp::FunctionArg],
        alias: &Option<sp::TableAlias>,
        lateral_left: Option<&Planned<'a>>,
        outer: Option<&'a Scope<'a>>,
    ) -> Result<Planned<'a>, SqlError> {
        let def = self
            .catalog
            .function(fname)
            .ok_or_else(|| SqlError::Unresolved {
                what: "function",
                name: fname.to_string(),
                hint: suggest(fname, self.catalog.functions().map(|f| f.name.as_str())),
            })?;
        let FunctionReturn::Table { schema } = &def.returns else {
            return Err(type_err(format!(
                "{fname} is a scalar function; use it in SELECT or WHERE, not FROM"
            )));
        };
        if def.volatility == Volatility::Volatile {
            return Err(unsupported(
                format!("{fname} in FROM"),
                format!("{fname} has side effects; run it with CALL {fname}(...) [FROM query]"),
            ));
        }
        let arg_scope: Scope<'_> = match lateral_left {
            Some(l) => Scope {
                items: l.scope.items.clone(),
                parent: outer,
            },
            None => Scope::empty(outer),
        };
        let mut resolved = vec![];
        for a in args {
            let e = match a {
                sp::FunctionArg::Unnamed(sp::FunctionArgExpr::Expr(e)) => e,
                _ => {
                    return Err(unsupported(
                        "named or wildcard table-function arguments",
                        "positional arguments",
                    ))
                }
            };
            resolved.push(self.resolve_expr(e, &arg_scope, false)?);
        }
        self.check_arity(&def.name, def.args.len(), def.variadic, resolved.len())?;
        let qualifier = alias_name(alias).unwrap_or_else(|| def.name.clone());
        let fn_schema: Schema = match alias {
            Some(a) if !a.columns.is_empty() => {
                if a.columns.len() != schema.len() {
                    return Err(type_err(format!(
                        "{} lists {} column names but {} returns {} columns",
                        ident_str(&a.name),
                        a.columns.len(),
                        def.name,
                        schema.len()
                    )));
                }
                Schema::new(
                    a.columns
                        .iter()
                        .zip(&schema.fields)
                        .map(|(c, f)| Field::new(ident_str(&c.name), f.data_type))
                        .collect(),
                )
            }
            _ => schema.clone(),
        };
        let mut items: Vec<ScopeItem> = match lateral_left {
            Some(l) => l.scope.items.clone(),
            None => vec![],
        };
        items.extend(items_from_schema(Some(&qualifier), &fn_schema));
        let plan = LogicalPlan::TableFunction {
            name: def.name.to_ascii_lowercase(),
            args: resolved,
            input: lateral_left.map(|l| Box::new(l.plan.clone())),
            schema: schema_of(&items),
        };
        Ok(Planned {
            plan,
            scope: Scope {
                items,
                parent: outer,
            },
        })
    }

    fn check_arity(
        &self,
        name: &str,
        declared: usize,
        variadic: bool,
        given: usize,
    ) -> Result<(), SqlError> {
        if given < declared || (!variadic && given > declared) {
            return Err(type_err(format!(
                "{name} takes {}{declared} argument{}, got {given}",
                if variadic { "at least " } else { "" },
                if declared == 1 { "" } else { "s" }
            )));
        }
        Ok(())
    }

    fn require_bool(&self, e: &Expr, scope: &Scope<'_>, ctx: &str) -> Result<(), SqlError> {
        let t = type_of(e, &scope.schema());
        if !is_bool_or_any(t) {
            return Err(type_err(format!(
                "{ctx} must be a boolean expression, got {t}"
            )));
        }
        Ok(())
    }

    // ------------------------------------------------------------ expressions

    /// Resolve an expression. `allow_aggregates` is true in SELECT / HAVING /
    /// ORDER BY contexts and false in WHERE, ON and GROUP BY.
    pub(crate) fn resolve_expr(
        &self,
        e: &sp::Expr,
        scope: &Scope<'_>,
        allow_aggregates: bool,
    ) -> Result<Expr, SqlError> {
        let schema = scope.schema();
        Ok(match e {
            sp::Expr::Identifier(id) => self.column(scope, None, &id.value)?,
            sp::Expr::CompoundIdentifier(parts) => match parts.as_slice() {
                [q, c] => self.column(scope, Some(&q.value), &c.value)?,
                _ => return Err(unsupported(format!("{e}"), "use table.column")),
            },
            sp::Expr::Nested(inner) => self.resolve_expr(inner, scope, allow_aggregates)?,
            sp::Expr::Value(v) => Expr::Literal(Literal(Self::literal(&v.value)?)),
            sp::Expr::TypedString(_) => {
                return Err(unsupported("typed string literals", "CAST('...' AS type)"))
            }
            sp::Expr::BinaryOp { left, op, right } => {
                let l = self.resolve_expr(left, scope, allow_aggregates)?;
                let r = self.resolve_expr(right, scope, allow_aggregates)?;
                let bop = match op {
                    sp::BinaryOperator::Plus => BinaryOp::Plus,
                    sp::BinaryOperator::Minus => BinaryOp::Minus,
                    sp::BinaryOperator::Multiply => BinaryOp::Multiply,
                    sp::BinaryOperator::Divide => BinaryOp::Divide,
                    sp::BinaryOperator::Modulo => BinaryOp::Modulo,
                    sp::BinaryOperator::StringConcat => BinaryOp::Concat,
                    sp::BinaryOperator::Gt => BinaryOp::Gt,
                    sp::BinaryOperator::Lt => BinaryOp::Lt,
                    sp::BinaryOperator::GtEq => BinaryOp::GtEq,
                    sp::BinaryOperator::LtEq => BinaryOp::LtEq,
                    sp::BinaryOperator::Eq => BinaryOp::Eq,
                    sp::BinaryOperator::NotEq => BinaryOp::NotEq,
                    sp::BinaryOperator::And => BinaryOp::And,
                    sp::BinaryOperator::Or => BinaryOp::Or,
                    other => {
                        return Err(unsupported(
                            format!("operator {other}"),
                            "supported: + - * / % || = <> < <= > >= AND OR LIKE ILIKE",
                        ))
                    }
                };
                let (lt, rt) = (type_of(&l, &schema), type_of(&r, &schema));
                match bop {
                    BinaryOp::And | BinaryOp::Or => {
                        if !is_bool_or_any(lt) || !is_bool_or_any(rt) {
                            return Err(type_err(format!(
                                "{bop:?} needs boolean operands, got {lt} and {rt}"
                            )));
                        }
                    }
                    BinaryOp::Plus
                    | BinaryOp::Minus
                    | BinaryOp::Multiply
                    | BinaryOp::Divide
                    | BinaryOp::Modulo
                        if !is_numeric_or_any(lt) || !is_numeric_or_any(rt) =>
                    {
                        return Err(type_err(format!(
                            "arithmetic needs numeric operands, got {lt} and {rt}; use || for text"
                        )));
                    }
                    _ => {}
                }
                Expr::Binary {
                    op: bop,
                    left: Box::new(l),
                    right: Box::new(r),
                }
            }
            sp::Expr::UnaryOp { op, expr } => {
                let operand = Box::new(self.resolve_expr(expr, scope, allow_aggregates)?);
                match op {
                    sp::UnaryOperator::Not => Expr::Unary {
                        op: UnaryOp::Not,
                        operand,
                    },
                    sp::UnaryOperator::Minus => Expr::Unary {
                        op: UnaryOp::Neg,
                        operand,
                    },
                    sp::UnaryOperator::Plus => *operand,
                    other => return Err(unsupported(format!("operator {other}"), "NOT or -")),
                }
            }
            sp::Expr::IsNull(inner) => Expr::Unary {
                op: UnaryOp::IsNull,
                operand: Box::new(self.resolve_expr(inner, scope, allow_aggregates)?),
            },
            sp::Expr::IsNotNull(inner) => Expr::Unary {
                op: UnaryOp::IsNotNull,
                operand: Box::new(self.resolve_expr(inner, scope, allow_aggregates)?),
            },
            sp::Expr::IsTrue(inner)
            | sp::Expr::IsFalse(inner)
            | sp::Expr::IsNotTrue(inner)
            | sp::Expr::IsNotFalse(inner) => {
                // x IS TRUE  == coalesce(x, false); x IS FALSE == coalesce(NOT x, false); the NOT forms negate.
                let x = self.resolve_expr(inner, scope, allow_aggregates)?;
                let base = match e {
                    sp::Expr::IsTrue(_) | sp::Expr::IsNotTrue(_) => x,
                    _ => Expr::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(x),
                    },
                };
                let coalesced = Expr::Function {
                    name: "coalesce".into(),
                    args: vec![base, Expr::Literal(Literal(Value::Bool(false)))],
                    data_type: DataType::Bool,
                };
                match e {
                    sp::Expr::IsNotTrue(_) | sp::Expr::IsNotFalse(_) => Expr::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(coalesced),
                    },
                    _ => coalesced,
                }
            }
            sp::Expr::Between {
                expr,
                negated,
                low,
                high,
            } => {
                let x = self.resolve_expr(expr, scope, allow_aggregates)?;
                let lo = self.resolve_expr(low, scope, allow_aggregates)?;
                let hi = self.resolve_expr(high, scope, allow_aggregates)?;
                let range = Expr::Binary {
                    op: BinaryOp::And,
                    left: Box::new(Expr::Binary {
                        op: BinaryOp::GtEq,
                        left: Box::new(x.clone()),
                        right: Box::new(lo),
                    }),
                    right: Box::new(Expr::Binary {
                        op: BinaryOp::LtEq,
                        left: Box::new(x),
                        right: Box::new(hi),
                    }),
                };
                if *negated {
                    Expr::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(range),
                    }
                } else {
                    range
                }
            }
            sp::Expr::Like {
                negated,
                expr,
                pattern,
                escape_char,
                any,
            }
            | sp::Expr::ILike {
                negated,
                expr,
                pattern,
                escape_char,
                any,
            } => {
                if escape_char.is_some() || *any {
                    return Err(unsupported(
                        "LIKE ... ESCAPE / LIKE ANY",
                        "plain LIKE with % and _",
                    ));
                }
                let op = if matches!(e, sp::Expr::Like { .. }) {
                    BinaryOp::Like
                } else {
                    BinaryOp::ILike
                };
                let like = Expr::Binary {
                    op,
                    left: Box::new(self.resolve_expr(expr, scope, allow_aggregates)?),
                    right: Box::new(self.resolve_expr(pattern, scope, allow_aggregates)?),
                };
                if *negated {
                    Expr::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(like),
                    }
                } else {
                    like
                }
            }
            sp::Expr::InList {
                expr,
                list,
                negated,
            } => Expr::InList {
                operand: Box::new(self.resolve_expr(expr, scope, allow_aggregates)?),
                list: list
                    .iter()
                    .map(|x| self.resolve_expr(x, scope, allow_aggregates))
                    .collect::<Result<_, _>>()?,
                negated: *negated,
            },
            sp::Expr::InSubquery {
                expr,
                subquery,
                negated,
            } => {
                let operand = self.resolve_expr(expr, scope, allow_aggregates)?;
                let sub = self.plan_query(subquery, Some(scope), &CteEnv::default())?;
                if sub.scope.items.len() != 1 {
                    return Err(type_err("IN (subquery) needs exactly one column"));
                }
                Expr::InSubquery {
                    operand: Box::new(operand),
                    subquery: Box::new(sub.plan),
                    negated: *negated,
                }
            }
            sp::Expr::Exists { subquery, negated } => {
                let sub = self.plan_query(subquery, Some(scope), &CteEnv::default())?;
                Expr::Exists {
                    subquery: Box::new(sub.plan),
                    negated: *negated,
                }
            }
            sp::Expr::Subquery(subquery) => {
                let sub = self.plan_query(subquery, Some(scope), &CteEnv::default())?;
                if sub.scope.items.len() != 1 {
                    return Err(type_err("a scalar subquery needs exactly one column"));
                }
                Expr::ScalarSubquery {
                    subquery: Box::new(sub.plan),
                }
            }
            sp::Expr::Cast {
                kind,
                expr,
                data_type,
                ..
            } => {
                if !matches!(kind, sp::CastKind::Cast | sp::CastKind::DoubleColon) {
                    return Err(unsupported("TRY_CAST / SAFE_CAST", "CAST(x AS type)"));
                }
                Expr::Cast {
                    operand: Box::new(self.resolve_expr(expr, scope, allow_aggregates)?),
                    to: Self::data_type(data_type)?,
                }
            }
            sp::Expr::Case {
                operand,
                conditions,
                else_result,
                ..
            } => {
                let op = operand
                    .as_ref()
                    .map(|o| self.resolve_expr(o, scope, allow_aggregates))
                    .transpose()?;
                let mut branches = vec![];
                for cw in conditions {
                    let mut cond = self.resolve_expr(&cw.condition, scope, allow_aggregates)?;
                    if let Some(o) = &op {
                        cond = Expr::Binary {
                            op: BinaryOp::Eq,
                            left: Box::new(o.clone()),
                            right: Box::new(cond),
                        };
                    }
                    let res = self.resolve_expr(&cw.result, scope, allow_aggregates)?;
                    branches.push((cond, res));
                }
                let otherwise = else_result
                    .as_ref()
                    .map(|o| self.resolve_expr(o, scope, allow_aggregates))
                    .transpose()?
                    .map(Box::new);
                Expr::Case {
                    branches,
                    otherwise,
                }
            }
            sp::Expr::Trim {
                trim_where,
                trim_what,
                expr,
                trim_characters,
            } => {
                if trim_what.is_some() || trim_characters.is_some() {
                    return Err(unsupported(
                        "TRIM with characters",
                        "trim(x), ltrim(x), rtrim(x)",
                    ));
                }
                let name = match trim_where {
                    Some(sp::TrimWhereField::Leading) => "ltrim",
                    Some(sp::TrimWhereField::Trailing) => "rtrim",
                    _ => "trim",
                };
                self.function_call(
                    name,
                    vec![self.resolve_expr(expr, scope, allow_aggregates)?],
                    &schema,
                )?
            }
            sp::Expr::Ceil { expr, field } | sp::Expr::Floor { expr, field } => {
                if !matches!(field, sp::CeilFloorKind::Scale(_))
                    && !matches!(
                        field,
                        sp::CeilFloorKind::DateTimeField(sp::DateTimeField::NoDateTime)
                    )
                {
                    return Err(unsupported("CEIL/FLOOR TO field", "ceil(x) / floor(x)"));
                }
                let name = if matches!(e, sp::Expr::Ceil { .. }) {
                    "ceil"
                } else {
                    "floor"
                };
                self.function_call(
                    name,
                    vec![self.resolve_expr(expr, scope, allow_aggregates)?],
                    &schema,
                )?
            }
            sp::Expr::Substring {
                expr,
                substring_from,
                substring_for,
                ..
            } => {
                let mut args = vec![self.resolve_expr(expr, scope, allow_aggregates)?];
                match substring_from {
                    Some(f) => args.push(self.resolve_expr(f, scope, allow_aggregates)?),
                    None => args.push(Expr::Literal(Literal(Value::Int(1)))),
                }
                if let Some(f) = substring_for {
                    args.push(self.resolve_expr(f, scope, allow_aggregates)?);
                }
                self.function_call("substr", args, &schema)?
            }
            sp::Expr::Function(f) => self.resolve_function(f, scope, allow_aggregates)?,
            sp::Expr::Tuple(_) => {
                return Err(unsupported("row values", "compare columns individually"))
            }
            other => {
                let head: String = other.to_string().chars().take(30).collect();
                return Err(unsupported(
                    format!("expression {head}"),
                    "see the CallSQL reference for supported expressions",
                ));
            }
        })
    }

    fn column(
        &self,
        scope: &Scope<'_>,
        qualifier: Option<&str>,
        name: &str,
    ) -> Result<Expr, SqlError> {
        let r = scope.resolve(qualifier, name)?;
        Ok(if r.depth == 0 {
            Expr::Column {
                index: r.index,
                name: name.to_string(),
            }
        } else {
            Expr::OuterColumn {
                depth: r.depth,
                index: r.index,
                name: name.to_string(),
            }
        })
    }

    fn literal(v: &sp::Value) -> Result<Value, SqlError> {
        Ok(match v {
            sp::Value::Number(n, _) => {
                if n.contains(['.', 'e', 'E']) {
                    Value::Float(n.parse().map_err(|_| type_err(format!("bad number {n}")))?)
                } else {
                    match n.parse::<i64>() {
                        Ok(i) => Value::Int(i),
                        Err(_) => Value::Float(
                            n.parse().map_err(|_| type_err(format!("bad number {n}")))?,
                        ),
                    }
                }
            }
            sp::Value::SingleQuotedString(s)
            | sp::Value::DoubleQuotedString(s)
            | sp::Value::EscapedStringLiteral(s)
            | sp::Value::UnicodeStringLiteral(s) => Value::Text(s.clone()),
            sp::Value::DollarQuotedString(d) => Value::Text(d.value.clone()),
            sp::Value::Boolean(b) => Value::Bool(*b),
            sp::Value::Null => Value::Null,
            other => {
                return Err(unsupported(
                    format!("literal {other}"),
                    "numbers, 'strings', TRUE, FALSE, NULL",
                ))
            }
        })
    }

    fn data_type(dt: &sp::DataType) -> Result<DataType, SqlError> {
        use sp::DataType as D;
        Ok(match dt {
            D::Boolean | D::Bool => DataType::Bool,
            D::Int(_)
            | D::Integer(_)
            | D::BigInt(_)
            | D::Int64
            | D::SmallInt(_)
            | D::TinyInt(_) => DataType::Int,
            D::Double(_)
            | D::DoublePrecision
            | D::Float(_)
            | D::Float64
            | D::Real
            | D::Float8
            | D::Float4 => DataType::Float,
            D::Text
            | D::Varchar(_)
            | D::String(_)
            | D::Char(_)
            | D::Character(_)
            | D::CharacterVarying(_) => DataType::Text,
            D::JSON => DataType::Json,
            D::Custom(n, _) => match object_name_last(n)?.to_ascii_lowercase().as_str() {
                "vector" => DataType::Vector,
                other => {
                    return Err(unsupported(
                        format!("type {other}"),
                        "BOOLEAN, BIGINT, DOUBLE, TEXT, JSON, VECTOR",
                    ))
                }
            },
            other => {
                return Err(unsupported(
                    format!("type {other}"),
                    "BOOLEAN, BIGINT, DOUBLE, TEXT, JSON, VECTOR",
                ))
            }
        })
    }

    fn function_call(
        &self,
        name: &str,
        args: Vec<Expr>,
        schema: &Schema,
    ) -> Result<Expr, SqlError> {
        let def = self
            .catalog
            .function(name)
            .ok_or_else(|| SqlError::Unresolved {
                what: "function",
                name: name.to_string(),
                hint: suggest(name, self.catalog.functions().map(|f| f.name.as_str())),
            })?;
        let FunctionReturn::Scalar { data_type } = &def.returns else {
            return Err(type_err(format!(
                "{} is a table function; use it in FROM",
                def.name
            )));
        };
        if def.volatility == Volatility::Volatile {
            return Err(unsupported(
                format!("{} in an expression", def.name),
                format!(
                    "{} has side effects; run it with CALL {}(...)",
                    def.name, def.name
                ),
            ));
        }
        self.check_arity(&def.name, def.args.len(), def.variadic, args.len())?;
        for (i, (a, want)) in args.iter().zip(&def.args).enumerate() {
            let got = type_of(a, schema);
            let ok = *want == DataType::Any
                || got == DataType::Any
                || got == *want
                || (*want == DataType::Float && got == DataType::Int)
                || (*want == DataType::Text);
            if !ok {
                return Err(type_err(format!(
                    "argument {} of {} should be {want}, got {got}",
                    i + 1,
                    def.name
                )));
            }
        }
        Ok(Expr::Function {
            name: def.name.to_ascii_lowercase(),
            args,
            data_type: *data_type,
        })
    }

    fn resolve_function(
        &self,
        f: &sp::Function,
        scope: &Scope<'_>,
        allow_aggregates: bool,
    ) -> Result<Expr, SqlError> {
        let name = object_name_last(&f.name)?.to_ascii_lowercase();
        if f.over.is_some() {
            return Err(unsupported(
                format!("window function {name}() OVER"),
                "aggregate in a subquery and join back",
            ));
        }
        if f.filter.is_some() || !f.within_group.is_empty() || f.null_treatment.is_some() {
            return Err(unsupported(
                "FILTER / WITHIN GROUP / IGNORE NULLS",
                "CASE inside the aggregate",
            ));
        }
        let (args, distinct, star) = match &f.args {
            sp::FunctionArguments::None => (vec![], false, false),
            sp::FunctionArguments::Subquery(_) => {
                return Err(unsupported(
                    "function over a subquery",
                    "pass a scalar subquery as an argument",
                ))
            }
            sp::FunctionArguments::List(list) => {
                if !list.clauses.is_empty() {
                    return Err(unsupported(
                        "ORDER BY / LIMIT inside a function call",
                        "plain arguments",
                    ));
                }
                let distinct = matches!(
                    list.duplicate_treatment,
                    Some(sp::DuplicateTreatment::Distinct)
                );
                let mut out = vec![];
                let mut star = false;
                for a in &list.args {
                    match a {
                        sp::FunctionArg::Unnamed(sp::FunctionArgExpr::Expr(e)) => out.push(e),
                        sp::FunctionArg::Unnamed(sp::FunctionArgExpr::Wildcard) => star = true,
                        _ => {
                            return Err(unsupported(
                                "named or qualified-wildcard arguments",
                                "positional arguments",
                            ))
                        }
                    }
                }
                (out, distinct, star)
            }
        };
        if let Some(agg) = aggregate_of(&name) {
            if !allow_aggregates {
                return Err(type_err(format!(
                    "aggregate {name}() is not allowed here; use HAVING or a subquery"
                )));
            }
            let resolved: Vec<Expr> = args
                .iter()
                .map(|a| self.resolve_expr(a, scope, false))
                .collect::<Result<_, _>>()?;
            if resolved.iter().any(contains_aggregate) {
                return Err(type_err("aggregate functions cannot be nested"));
            }
            let expected = match agg {
                AggregateFn::Count => {
                    if star {
                        0
                    } else {
                        1
                    }
                }
                AggregateFn::StringAgg => 2,
                _ => 1,
            };
            if star && agg != AggregateFn::Count {
                return Err(type_err(format!("{name}(*) is only valid for COUNT")));
            }
            if resolved.len() != expected {
                return Err(type_err(format!(
                    "{name} takes {expected} argument{}, got {}",
                    if expected == 1 { "" } else { "s" },
                    resolved.len()
                )));
            }
            return Ok(Expr::Aggregate {
                func: agg,
                args: resolved,
                distinct,
            });
        }
        if star {
            return Err(type_err(format!(
                "{name}(*) is not valid; only COUNT(*) takes *"
            )));
        }
        let resolved: Vec<Expr> = args
            .iter()
            .map(|a| self.resolve_expr(a, scope, allow_aggregates))
            .collect::<Result<_, _>>()?;
        self.function_call(&name, resolved, &scope.schema())
    }
}
