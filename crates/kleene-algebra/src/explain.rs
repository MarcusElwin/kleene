//! `EXPLAIN` rendering.

use crate::{CallNode, CallPlan};
use kleene_core::{CallKind, Volatility};
use kleene_sql::{Expr, JoinKind, LogicalPlan};

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
        LogicalPlan::Recursive {
            name,
            all,
            recursive,
            max_rounds,
            ..
        } => {
            let mut s = format!("μ {name}{}", if *all { " (bag)" } else { "" });
            if let Some(k) = crate::annotate::beam_of(recursive) {
                s += &format!(" beam {k}");
            }
            if let Some(r) = max_rounds {
                s += &format!(" ≤{r} rounds");
            }
            s
        }
    }
}

fn expr_label(e: &Expr) -> String {
    match e {
        Expr::Column { name, .. } => name.clone(),
        Expr::OuterColumn { name, depth, .. } => format!("outer{depth}.{name}"),
        Expr::Literal(l) => match &l.0 {
            kleene_core::Value::Text(s) => format!(
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
                kleene_sql::BinaryOp::Eq => "=",
                kleene_sql::BinaryOp::NotEq => "<>",
                kleene_sql::BinaryOp::Lt => "<",
                kleene_sql::BinaryOp::LtEq => "<=",
                kleene_sql::BinaryOp::Gt => ">",
                kleene_sql::BinaryOp::GtEq => ">=",
                kleene_sql::BinaryOp::And => "AND",
                kleene_sql::BinaryOp::Or => "OR",
                kleene_sql::BinaryOp::Plus => "+",
                kleene_sql::BinaryOp::Minus => "-",
                kleene_sql::BinaryOp::Multiply => "*",
                kleene_sql::BinaryOp::Divide => "/",
                kleene_sql::BinaryOp::Modulo => "%",
                kleene_sql::BinaryOp::Concat => "||",
                kleene_sql::BinaryOp::Like => "LIKE",
                kleene_sql::BinaryOp::ILike => "ILIKE",
            };
            format!("{} {sym} {}", expr_label(left), expr_label(right))
        }
        Expr::Unary { op, operand } => match op {
            kleene_sql::UnaryOp::Not => format!("NOT {}", expr_label(operand)),
            kleene_sql::UnaryOp::Neg => format!("-{}", expr_label(operand)),
            kleene_sql::UnaryOp::IsNull => format!("{} IS NULL", expr_label(operand)),
            kleene_sql::UnaryOp::IsNotNull => format!("{} IS NOT NULL", expr_label(operand)),
        },
        Expr::Function { name, args, .. } => format!(
            "{name}({})",
            args.iter().map(expr_label).collect::<Vec<_>>().join(", ")
        ),
        Expr::Aggregate {
            func,
            args,
            distinct,
            order,
        } => {
            let (plain, keys) = args.split_at(args.len() - order.len());
            let keys: Vec<String> = keys
                .iter()
                .zip(order)
                .map(|(k, o)| format!("{}{}", expr_label(k), if o.asc { "" } else { " desc" }))
                .collect();
            format!(
                "{:?}({}{}{})",
                func,
                if *distinct { "DISTINCT " } else { "" },
                if plain.is_empty() {
                    "*".to_string()
                } else {
                    plain.iter().map(expr_label).collect::<Vec<_>>().join(", ")
                },
                if keys.is_empty() {
                    String::new()
                } else {
                    format!(" order by {}", keys.join(", "))
                }
            )
            .to_lowercase()
        }
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
        CallKind::LlmScalar { alias, batch: None } => format!("λ {}", alias.0),
        CallKind::LlmScalar {
            alias,
            batch: Some(b),
        } => format!("λ {} ×{b}", alias.0),
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
    if let Some(ps) = &plan.plan_space {
        out.push_str(&format!(
            "plan space: {} relations, {} join orders, {} splits priced, {} pruned\n",
            ps.relations, ps.orders, ps.evaluated, ps.pruned
        ));
    }
    if !plan.learned.is_empty() {
        out.push_str(&format!("learned: {}\n", plan.learned.join("; ")));
    }
    if !plan.alternatives.is_empty() {
        out.push_str("alternatives:\n");
        for a in &plan.alternatives {
            out.push_str(&format!(
                "  {} {}: ~{} calls, ~${:.4}, ~{} rows\n",
                if a.chosen { "▶" } else { " " },
                a.label,
                fmt_num(a.estimate.calls),
                a.estimate.dollars,
                fmt_num(a.estimate.rows)
            ));
        }
    }
    out
}
