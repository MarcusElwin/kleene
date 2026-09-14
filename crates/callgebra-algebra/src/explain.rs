//! `EXPLAIN` rendering.

use crate::{CallNode, CallPlan};
use callgebra_core::{CallKind, Volatility};
use callgebra_sql::{Expr, JoinKind, LogicalPlan};

fn op_label(op: &LogicalPlan) -> String {
    match op {
        LogicalPlan::Scan { table, .. } => format!("scan {table}"),
        LogicalPlan::Values { rows, .. } => format!("values ({} rows)", rows.len()),
        LogicalPlan::CteRef { name, .. } => format!("cte {name}"),
        LogicalPlan::Project { exprs, .. } => format!(
            "π {}",
            exprs.iter().map(expr_label).collect::<Vec<_>>().join(", ")
        ),
        LogicalPlan::Filter { predicate, .. } => format!("σ {}", expr_label(predicate)),
        LogicalPlan::Join { kind, on, .. } => {
            let sym = match kind {
                JoinKind::Inner => "⋈",
                JoinKind::Left => "⟕",
                JoinKind::Cross => "×",
                JoinKind::Semi => "⋉",
                JoinKind::Anti => "▷",
            };
            match on {
                Some(e) => format!("{sym} {}", expr_label(e)),
                None => sym.to_string(),
            }
        }
        LogicalPlan::TableFunction {
            name, args, input, ..
        } => format!(
            "{}{}({})",
            if input.is_some() { "κ lateral " } else { "" },
            name,
            args.iter().map(expr_label).collect::<Vec<_>>().join(", ")
        ),
        LogicalPlan::Aggregate {
            group_by,
            aggregates,
            ..
        } => format!(
            "γ [{}] {}",
            group_by
                .iter()
                .map(expr_label)
                .collect::<Vec<_>>()
                .join(", "),
            aggregates
                .iter()
                .map(expr_label)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        LogicalPlan::Sort { keys, .. } => format!(
            "sort {}",
            keys.iter()
                .map(|k| format!(
                    "{}{}",
                    expr_label(&k.expr),
                    if k.asc { "" } else { " desc" }
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        LogicalPlan::Limit { offset, limit, .. } => match limit {
            Some(l) => format!("limit {l} offset {offset}"),
            None => format!("offset {offset}"),
        },
        LogicalPlan::Distinct { .. } => "distinct".into(),
        LogicalPlan::Union { all, .. } => if *all { "∪ all" } else { "∪" }.into(),
        LogicalPlan::With { ctes, .. } => format!(
            "with {}",
            ctes.iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        LogicalPlan::Recursive { name, all, .. } => {
            format!("μ {name}{}", if *all { " (bag)" } else { "" })
        }
    }
}

fn expr_label(e: &Expr) -> String {
    match e {
        Expr::Column { name, .. } => name.clone(),
        Expr::OuterColumn { name, depth, .. } => format!("outer{depth}.{name}"),
        Expr::Literal(l) => match &l.0 {
            callgebra_core::Value::Text(s) => format!(
                "'{}'",
                if s.chars().count() > 20 {
                    format!("{}…", s.chars().take(20).collect::<String>())
                } else {
                    s.clone()
                }
            ),
            other => other.render(),
        },
        Expr::Binary { op, left, right } => {
            let sym = match op {
                callgebra_sql::BinaryOp::Eq => "=",
                callgebra_sql::BinaryOp::NotEq => "<>",
                callgebra_sql::BinaryOp::Lt => "<",
                callgebra_sql::BinaryOp::LtEq => "<=",
                callgebra_sql::BinaryOp::Gt => ">",
                callgebra_sql::BinaryOp::GtEq => ">=",
                callgebra_sql::BinaryOp::And => "AND",
                callgebra_sql::BinaryOp::Or => "OR",
                callgebra_sql::BinaryOp::Plus => "+",
                callgebra_sql::BinaryOp::Minus => "-",
                callgebra_sql::BinaryOp::Multiply => "*",
                callgebra_sql::BinaryOp::Divide => "/",
                callgebra_sql::BinaryOp::Modulo => "%",
                callgebra_sql::BinaryOp::Concat => "||",
                callgebra_sql::BinaryOp::Like => "LIKE",
                callgebra_sql::BinaryOp::ILike => "ILIKE",
            };
            format!("{} {sym} {}", expr_label(left), expr_label(right))
        }
        Expr::Unary { op, operand } => match op {
            callgebra_sql::UnaryOp::Not => format!("NOT {}", expr_label(operand)),
            callgebra_sql::UnaryOp::Neg => format!("-{}", expr_label(operand)),
            callgebra_sql::UnaryOp::IsNull => format!("{} IS NULL", expr_label(operand)),
            callgebra_sql::UnaryOp::IsNotNull => format!("{} IS NOT NULL", expr_label(operand)),
        },
        Expr::Function { name, args, .. } => format!(
            "{name}({})",
            args.iter().map(expr_label).collect::<Vec<_>>().join(", ")
        ),
        Expr::Aggregate {
            func,
            args,
            distinct,
        } => format!(
            "{:?}({}{})",
            func,
            if *distinct { "DISTINCT " } else { "" },
            if args.is_empty() {
                "*".to_string()
            } else {
                args.iter().map(expr_label).collect::<Vec<_>>().join(", ")
            }
        )
        .to_lowercase(),
        Expr::Cast { operand, to } => format!("cast({} as {to})", expr_label(operand)),
        Expr::Case { .. } => "case".into(),
        Expr::Exists { negated, .. } => if *negated {
            "NOT EXISTS (…)"
        } else {
            "EXISTS (…)"
        }
        .into(),
        Expr::InSubquery {
            operand, negated, ..
        } => format!(
            "{}{} IN (…)",
            expr_label(operand),
            if *negated { " NOT" } else { "" }
        ),
        Expr::ScalarSubquery { .. } => "(…)".into(),
        Expr::InList {
            operand,
            list,
            negated,
        } => format!(
            "{}{} IN ({} values)",
            expr_label(operand),
            if *negated { " NOT" } else { "" },
            list.len()
        ),
    }
}

fn kind_label(k: &CallKind) -> String {
    match k {
        CallKind::Pure => "pure".into(),
        CallKind::LlmScalar { alias } => format!("λ {}", alias.0),
        CallKind::LlmTable { alias } => format!("κ {}", alias.0),
        CallKind::Tool { tool } => format!("tool {tool}"),
        CallKind::Recursive { role } => format!("ρ {role}"),
    }
}

fn render_node(n: &CallNode, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    let e = &n.estimate;
    let mut line = format!("{indent}{}", op_label(&n.op));
    if !n.calls.is_empty() {
        line += &format!(
            "  [{}]",
            n.calls
                .iter()
                .map(kind_label)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if n.volatility == Volatility::Volatile {
        line += "  ‖ fence";
    }
    line += &format!("  ~{} rows", fmt_num(e.rows));
    if e.calls > 0.0 {
        line += &format!(
            ", {} calls, {} tok, ${:.4}",
            fmt_num(e.calls),
            fmt_num(e.tokens),
            e.dollars
        );
    }
    out.push_str(&line);
    out.push('\n');
    for c in &n.children {
        render_node(c, depth + 1, out);
    }
}

fn fmt_num(x: f64) -> String {
    if x >= 1000.0 {
        format!("{:.1}k", x / 1000.0)
    } else if x.fract() == 0.0 {
        format!("{x:.0}")
    } else {
        format!("{x:.1}")
    }
}

/// Render a call plan as an indented tree with per-node estimates, followed
/// by totals, the fragment badge and the rules that fired.
pub fn explain(plan: &CallPlan) -> String {
    let mut out = String::new();
    render_node(&plan.root, 0, &mut out);
    let t = &plan.total;
    out.push_str(&format!(
        "total: ~{} calls, ~{} tokens, ~${:.4}, depth {}\n",
        fmt_num(t.calls),
        fmt_num(t.tokens),
        t.dollars,
        t.depth
    ));
    out.push_str(&format!(
        "fragment: {} ({})\n",
        plan.fragment.badge(),
        plan.fragment.note()
    ));
    if !plan.rules_applied.is_empty() {
        out.push_str(&format!("rules: {}\n", plan.rules_applied.join(", ")));
    }
    out
}
