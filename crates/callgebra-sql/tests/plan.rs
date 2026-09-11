//! Planner tests over a fixed catalog.

use callgebra_core::{
    standard_catalog, Catalog, DataType, Field, Schema, TableDef, TableSource, Volatility,
};
use callgebra_sql::{
    plan_sql, render_error, type_of, AggregateFn, BinaryOp, Expr, JoinKind, LogicalPlan, SqlError,
    Statement, StatementKind,
};

fn table(name: &str, cols: &[(&str, DataType)]) -> TableDef {
    TableDef {
        name: name.into(),
        schema: Schema::new(cols.iter().map(|(n, t)| Field::new(*n, *t)).collect()),
        source: TableSource::Stored,
        volatility: Volatility::Stable,
        description: String::new(),
    }
}

fn catalog() -> Catalog {
    let mut c = standard_catalog();
    c.add_function(callgebra_core::FunctionDef {
        name: "shell".into(),
        args: vec![DataType::Text],
        variadic: true,
        returns: callgebra_core::FunctionReturn::Table {
            schema: Schema::new(vec![
                Field::new("stdout", DataType::Text),
                Field::new("exit_code", DataType::Int),
            ]),
        },
        call_kind: callgebra_core::CallKind::Tool {
            tool: "shell".into(),
        },
        volatility: Volatility::Volatile,
        description: "run a command".into(),
    });
    c.add_table(table(
        "edges",
        &[
            ("src", DataType::Int),
            ("dst", DataType::Int),
            ("w", DataType::Float),
        ],
    ));
    c.add_table(table(
        "nodes",
        &[("id", DataType::Int), ("name", DataType::Text)],
    ));
    c.add_table(table(
        "docs",
        &[
            ("id", DataType::Int),
            ("body", DataType::Text),
            ("meta", DataType::Json),
        ],
    ));
    c
}

fn one(sql: &str) -> Statement {
    let mut v = plan_sql(sql, &catalog()).unwrap_or_else(|e| panic!("{sql}\n{}", render_error(&e)));
    assert_eq!(v.len(), 1);
    v.remove(0)
}

fn query(sql: &str) -> LogicalPlan {
    match one(sql).kind {
        StatementKind::Query { plan } => plan,
        other => panic!("expected query, got {other:?}"),
    }
}

fn err(sql: &str) -> SqlError {
    plan_sql(sql, &catalog()).expect_err(sql)
}

fn names(p: &LogicalPlan) -> Vec<String> {
    p.schema().names().into_iter().map(String::from).collect()
}

/// Strip trailing Limit/Sort/Distinct/Project wrappers down to the first node of a kind.
fn find<'a>(p: &'a LogicalPlan, pred: &dyn Fn(&LogicalPlan) -> bool) -> Option<&'a LogicalPlan> {
    if pred(p) {
        return Some(p);
    }
    p.children().into_iter().find_map(|c| find(c, pred))
}

#[test]
fn simple_select_where_order_limit() {
    let p = query("SELECT id, name AS n FROM nodes WHERE id > 3 ORDER BY n DESC LIMIT 5 OFFSET 2");
    assert_eq!(names(&p), ["id", "n"]);
    let LogicalPlan::Limit {
        offset: 2,
        limit: Some(5),
        input,
    } = &p
    else {
        panic!("{p:?}")
    };
    let LogicalPlan::Sort { keys, input } = &**input else {
        panic!("{input:?}")
    };
    assert_eq!(keys.len(), 1);
    assert!(!keys[0].asc);
    assert!(keys[0].nulls_first, "DESC defaults to NULLS FIRST");
    assert!(matches!(keys[0].expr, Expr::Column { index: 1, .. }));
    let LogicalPlan::Project { input, .. } = &**input else {
        panic!()
    };
    let LogicalPlan::Filter { predicate, .. } = &**input else {
        panic!()
    };
    assert!(matches!(
        predicate,
        Expr::Binary {
            op: BinaryOp::Gt,
            ..
        }
    ));
}

#[test]
fn star_and_qualified_star_expand_in_order() {
    let p = query("SELECT * FROM nodes");
    assert_eq!(names(&p), ["id", "name"]);
    let p = query("SELECT e.*, n.name FROM edges e JOIN nodes n ON n.id = e.dst");
    assert_eq!(names(&p), ["src", "dst", "w", "name"]);
}

#[test]
fn joins_resolve_positions_and_reject_ambiguity() {
    let p = query("SELECT n.name, e.w FROM edges e LEFT JOIN nodes n ON n.id = e.src");
    let j = find(&p, &|x| matches!(x, LogicalPlan::Join { .. })).unwrap();
    let LogicalPlan::Join {
        kind,
        on: Some(on),
        schema,
        ..
    } = j
    else {
        panic!()
    };
    assert_eq!(*kind, JoinKind::Left);
    assert_eq!(schema.names(), ["src", "dst", "w", "id", "name"]);
    // n.id is position 3 in left++right, e.src is 0.
    let Expr::Binary { left, right, .. } = on else {
        panic!()
    };
    assert!(matches!(**left, Expr::Column { index: 3, .. }));
    assert!(matches!(**right, Expr::Column { index: 0, .. }));

    let e = err("SELECT id FROM nodes a JOIN nodes b ON a.id = b.id");
    match e {
        SqlError::Unresolved {
            what: "column",
            hint: Some(h),
            ..
        } => assert!(h.contains("ambiguous")),
        other => panic!("{other:?}"),
    }
    let p = query("SELECT a.id FROM nodes a, nodes b");
    let j = find(&p, &|x| {
        matches!(
            x,
            LogicalPlan::Join {
                kind: JoinKind::Cross,
                ..
            }
        )
    })
    .unwrap();
    assert!(matches!(j, LogicalPlan::Join { on: None, .. }));
}

#[test]
fn group_by_having_and_count_star() {
    let p = query(
        "SELECT src, COUNT(*) AS n, SUM(w) FROM edges GROUP BY src HAVING COUNT(*) > 1 ORDER BY n",
    );
    assert_eq!(names(&p), ["src", "n", "sum"]);
    let agg = find(&p, &|x| matches!(x, LogicalPlan::Aggregate { .. })).unwrap();
    let LogicalPlan::Aggregate {
        group_by,
        aggregates,
        schema,
        ..
    } = agg
    else {
        panic!()
    };
    assert_eq!(group_by.len(), 1);
    assert_eq!(
        aggregates.len(),
        2,
        "COUNT(*) is shared between SELECT and HAVING"
    );
    assert!(matches!(
        aggregates[0],
        Expr::Aggregate {
            func: AggregateFn::Count,
            ..
        }
    ));
    assert_eq!(schema.fields[1].data_type, DataType::Int);
    assert_eq!(schema.fields[2].data_type, DataType::Float);
    // HAVING is a Filter over the Aggregate.
    let f = find(&p, &|x| matches!(x, LogicalPlan::Filter { .. })).unwrap();
    let LogicalPlan::Filter { input, .. } = f else {
        panic!()
    };
    assert!(matches!(**input, LogicalPlan::Aggregate { .. }));

    let e = err("SELECT src, dst FROM edges GROUP BY src");
    assert!(matches!(e, SqlError::Type { .. }), "{e:?}");
    let e = err("SELECT COUNT(*) FROM edges WHERE COUNT(*) > 1");
    assert!(matches!(e, SqlError::Type { .. }));
    // group by ordinal and alias
    query("SELECT src AS s, COUNT(*) FROM edges GROUP BY 1");
    query("SELECT src + 1 AS s, COUNT(*) FROM edges GROUP BY s");
}

#[test]
fn distinct_union_and_union_all() {
    let p = query("SELECT DISTINCT src FROM edges");
    assert!(matches!(p, LogicalPlan::Distinct { .. }));
    let p =
        query("SELECT src FROM edges UNION SELECT dst FROM edges UNION ALL SELECT id FROM nodes");
    let LogicalPlan::Union { inputs, all } = &p else {
        panic!("{p:?}")
    };
    assert!(*all);
    assert_eq!(inputs.len(), 2);
    let e = err("SELECT src, dst FROM edges UNION SELECT id FROM nodes");
    assert!(matches!(e, SqlError::Type { .. }));
    let e = err("SELECT src FROM edges EXCEPT SELECT id FROM nodes");
    assert!(matches!(e, SqlError::Unsupported { .. }));
}

#[test]
fn types_follow_duckdb_rules() {
    let p = query("SELECT src / 2, src + 1, w * 2, upper(name) FROM edges JOIN nodes ON id = src");
    let s = p.schema();
    assert_eq!(
        s.fields[0].data_type,
        DataType::Float,
        "/ is float division"
    );
    assert_eq!(s.fields[1].data_type, DataType::Int);
    assert_eq!(s.fields[2].data_type, DataType::Float);
    assert_eq!(s.fields[3].data_type, DataType::Text);
    let p = query("SELECT CAST(src AS TEXT), CASE WHEN w > 1 THEN 'big' ELSE 'small' END, src BETWEEN 1 AND 3, name LIKE 'a%', src IN (1, 2) FROM edges, nodes");
    let s = p.schema();
    assert_eq!(s.fields[0].data_type, DataType::Text);
    assert_eq!(s.fields[1].data_type, DataType::Text);
    assert_eq!(s.fields[2].data_type, DataType::Bool);
    assert_eq!(s.fields[3].data_type, DataType::Bool);
    assert_eq!(s.fields[4].data_type, DataType::Bool);
    let e = err("SELECT name + 1 FROM nodes");
    assert!(matches!(e, SqlError::Type { .. }));
    let e = err("SELECT * FROM nodes WHERE name");
    assert!(matches!(e, SqlError::Type { .. }));
}

#[test]
fn exists_and_in_subqueries_are_correlated_via_outer_columns() {
    let p = query(
        "SELECT n.id FROM nodes n WHERE NOT EXISTS (SELECT 1 FROM edges e WHERE e.src = n.id)",
    );
    let f = find(&p, &|x| matches!(x, LogicalPlan::Filter { .. })).unwrap();
    let LogicalPlan::Filter {
        predicate: Expr::Exists {
            subquery,
            negated: true,
        },
        ..
    } = f
    else {
        panic!("{f:?}")
    };
    let inner = find(subquery, &|x| matches!(x, LogicalPlan::Filter { .. })).unwrap();
    let LogicalPlan::Filter {
        predicate: Expr::Binary { left, right, .. },
        ..
    } = inner
    else {
        panic!()
    };
    assert!(
        matches!(**left, Expr::Column { index: 0, .. }),
        "e.src is column 0 of edges"
    );
    assert!(
        matches!(
            **right,
            Expr::OuterColumn {
                depth: 1,
                index: 0,
                ..
            }
        ),
        "n.id is column 0 one level up: {right:?}"
    );

    let p = query("SELECT id FROM nodes WHERE id IN (SELECT src FROM edges)");
    let f = find(&p, &|x| matches!(x, LogicalPlan::Filter { .. })).unwrap();
    assert!(matches!(
        f,
        LogicalPlan::Filter {
            predicate: Expr::InSubquery { negated: false, .. },
            ..
        }
    ));
    let e = err("SELECT id FROM nodes WHERE id IN (SELECT src, dst FROM edges)");
    assert!(matches!(e, SqlError::Type { .. }));

    let p = query("SELECT id, (SELECT MAX(w) FROM edges WHERE src = nodes.id) AS best FROM nodes");
    assert_eq!(p.schema().fields[1].data_type, DataType::Float);
}

#[test]
fn with_and_recursive_cte() {
    let p = query("WITH a AS (SELECT src FROM edges), b AS (SELECT src FROM a) SELECT * FROM b");
    let LogicalPlan::With { ctes, .. } = &p else {
        panic!("{p:?}")
    };
    assert_eq!(ctes.len(), 2);
    let p = query(
        "WITH RECURSIVE reach(n) AS (SELECT dst FROM edges WHERE src = 1 UNION SELECT e.dst FROM reach r JOIN edges e ON e.src = r.n) SELECT n FROM reach ORDER BY n",
    );
    let LogicalPlan::Recursive {
        name,
        all,
        recursive,
        body,
        ..
    } = &p
    else {
        panic!("{p:?}")
    };
    assert_eq!(name, "reach");
    assert!(!all);
    assert_eq!(names(body), ["n"]);
    let cref = find(recursive, &|x| matches!(x, LogicalPlan::CteRef { .. })).unwrap();
    assert_eq!(names(cref), ["n"]);
    // A WITH RECURSIVE whose CTE never references itself is ordinary SQL.
    query("WITH RECURSIVE r AS (SELECT 1 AS x) SELECT * FROM r UNION SELECT 2");
    let e = err("WITH RECURSIVE r(x) AS (SELECT 1 UNION SELECT x, x FROM r) SELECT * FROM r");
    assert!(matches!(e, SqlError::Type { .. }), "{e:?}");
    let e = err("WITH RECURSIVE a AS (SELECT 1 AS x UNION SELECT x FROM b), b AS (SELECT x FROM a) SELECT * FROM a");
    assert!(
        matches!(
            e,
            SqlError::Unresolved { .. } | SqlError::Unsupported { .. }
        ),
        "{e:?}"
    );
}

#[test]
fn table_functions_plain_and_lateral() {
    let p = query("SELECT generate_series FROM generate_series(1, 3)");
    let tf = find(&p, &|x| matches!(x, LogicalPlan::TableFunction { .. })).unwrap();
    assert!(matches!(tf, LogicalPlan::TableFunction { input: None, .. }));
    let p = query("SELECT n.id, g.generate_series AS k FROM nodes n CROSS JOIN LATERAL generate_series(1, n.id) AS g");
    assert_eq!(names(&p), ["id", "k"]);
    let tf = find(&p, &|x| matches!(x, LogicalPlan::TableFunction { .. })).unwrap();
    let LogicalPlan::TableFunction {
        input: Some(_),
        args,
        schema,
        ..
    } = tf
    else {
        panic!()
    };
    assert!(matches!(args[1], Expr::Column { index: 0, .. }));
    assert_eq!(schema.names(), ["id", "name", "generate_series"]);
    let e = err("SELECT * FROM upper('x')");
    assert!(matches!(e, SqlError::Type { .. }), "{e:?}");
    let e = err("SELECT generate_series(1, 2) FROM nodes");
    assert!(matches!(e, SqlError::Type { .. }), "{e:?}");
}

#[test]
fn insert_create_drop_explain_set() {
    let s = one("INSERT INTO nodes (name, id) VALUES ('a', 1), ('b', 2)");
    let StatementKind::Insert { table, plan } = s.kind else {
        panic!()
    };
    assert_eq!(table, "nodes");
    assert_eq!(names(&plan), ["id", "name"], "reordered into table order");
    let s = one("INSERT INTO nodes (id) VALUES (3)");
    let StatementKind::Insert { plan, .. } = s.kind else {
        panic!()
    };
    let LogicalPlan::Project { exprs, .. } = &plan else {
        panic!()
    };
    assert!(
        matches!(exprs[1], Expr::Literal(_)),
        "missing column becomes NULL"
    );
    let e = err("INSERT INTO nodes VALUES (1)");
    match e {
        SqlError::Type { message } => assert!(message.contains("expects 2 columns")),
        other => panic!("{other:?}"),
    }
    let s = one("CREATE TABLE IF NOT EXISTS t AS SELECT id FROM nodes");
    assert!(matches!(
        s.kind,
        StatementKind::CreateTableAs {
            if_not_exists: true,
            ..
        }
    ));
    let s = one("DROP TABLE IF EXISTS t");
    assert!(matches!(
        s.kind,
        StatementKind::DropTable {
            if_exists: true,
            ..
        }
    ));
    let s = one("EXPLAIN ANALYZE SELECT 1");
    assert!(matches!(
        s.kind,
        StatementKind::Explain { analyze: true, .. }
    ));
    let s = one("SET budget.calls = 200");
    let StatementKind::Set { key, value } = s.kind else {
        panic!()
    };
    assert_eq!((key.as_str(), value.as_str()), ("budget.calls", "200"));
    let s = one("SET effort = 'low'");
    let StatementKind::Set { value, .. } = s.kind else {
        panic!()
    };
    assert_eq!(value, "low");
}

#[test]
fn final_forms() {
    let s = one("FINAL('done')");
    let StatementKind::Final { plan } = s.kind else {
        panic!()
    };
    assert_eq!(names(&plan), ["answer"]);
    let s = one("FINAL FROM (SELECT id FROM nodes WHERE id = 1);");
    let StatementKind::Final { plan } = s.kind else {
        panic!()
    };
    assert_eq!(names(&plan), ["id"]);
}

#[test]
fn unsupported_constructs_have_hints() {
    for (sql, needle) in [
        (
            "SELECT * FROM edges RIGHT JOIN nodes ON id = src",
            "LEFT JOIN",
        ),
        ("SELECT row_number() OVER () FROM nodes", "window"),
        ("CREATE AGENT reviewer MODEL 'worker' PROMPT 'x'", "M3"),
        (
            "CREATE FUNCTION f(x TEXT) RETURNS TEXT AS PROMPT 'hi'",
            "M2",
        ),
        ("CALL shell('ls')", "M2"),
        ("SELECT * FROM edges NATURAL JOIN nodes", "ON"),
        ("UPDATE nodes SET name = 'x'", "append-only"),
    ] {
        let e = err(sql);
        let rendered = render_error(&e);
        assert!(rendered.contains(needle), "{sql}: {rendered}");
    }
}

#[test]
fn unknown_names_suggest_alternatives() {
    let e = err("SELECT uppr(name) FROM nodes");
    assert!(render_error(&e).contains("upper"), "{}", render_error(&e));
    let e = err("SELECT * FROM node");
    assert!(render_error(&e).contains("nodes"));
    let e = err("SELECT nam FROM nodes");
    assert!(render_error(&e).contains("name"));
}

#[test]
fn order_by_non_projected_column_adds_and_strips_extras() {
    let p = query("SELECT name FROM nodes ORDER BY id");
    assert_eq!(names(&p), ["name"]);
    let LogicalPlan::Project { input, .. } = &p else {
        panic!("{p:?}")
    };
    let LogicalPlan::Sort { input, .. } = &**input else {
        panic!()
    };
    assert_eq!(names(input), ["name", "order1"]);
    let e = err("SELECT DISTINCT name FROM nodes ORDER BY id");
    assert!(matches!(e, SqlError::Unsupported { .. }));
    query("SELECT name FROM nodes ORDER BY 1");
    let e = err("SELECT name FROM nodes ORDER BY 2");
    assert!(matches!(e, SqlError::Type { .. }));
}

#[test]
fn type_of_direct() {
    let schema = Schema::new(vec![
        Field::new("a", DataType::Int),
        Field::new("b", DataType::Float),
    ]);
    let col = |i, n: &str| Expr::Column {
        index: i,
        name: n.into(),
    };
    let bin = |op, l: Expr, r: Expr| Expr::Binary {
        op,
        left: Box::new(l),
        right: Box::new(r),
    };
    assert_eq!(
        type_of(&bin(BinaryOp::Plus, col(0, "a"), col(0, "a")), &schema),
        DataType::Int
    );
    assert_eq!(
        type_of(&bin(BinaryOp::Divide, col(0, "a"), col(0, "a")), &schema),
        DataType::Float
    );
    assert_eq!(
        type_of(&bin(BinaryOp::Plus, col(0, "a"), col(1, "b")), &schema),
        DataType::Float
    );
    assert_eq!(
        type_of(&bin(BinaryOp::Concat, col(0, "a"), col(1, "b")), &schema),
        DataType::Text
    );
}

#[test]
fn multiple_statements_keep_their_text() {
    let v = plan_sql("SELECT 1; SELECT 2", &catalog()).unwrap();
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].sql, "SELECT 1");
    let v = plan_sql("  SELECT 1 ; ", &catalog()).unwrap();
    assert_eq!(v[0].sql, "SELECT 1 ;");
}

#[test]
fn create_function_and_call_statements() {
    let s = one("CREATE FUNCTION verify(c TEXT) RETURNS BOOLEAN AS PROMPT 'Is {c} right?'");
    assert!(matches!(s.kind, StatementKind::CreateFunction { .. }));
    let s = one("CALL shell('ls -la')");
    let StatementKind::Call { tool, args, input } = s.kind else {
        panic!()
    };
    assert_eq!(tool, "shell");
    assert_eq!(args.len(), 1);
    assert!(input.is_none());
    let s =
        one("CALL shell('cat ' || name, 'tmp') FROM (SELECT name FROM nodes WHERE id < 3) AS q");
    let StatementKind::Call {
        args,
        input: Some(input),
        ..
    } = s.kind
    else {
        panic!()
    };
    assert_eq!(args.len(), 2);
    assert!(matches!(
        args[0],
        Expr::Binary {
            op: BinaryOp::Concat,
            ..
        }
    ));
    assert_eq!(names(&input), ["name"]);
    let e = err("CALL upper('x')");
    assert!(matches!(e, SqlError::Unsupported { .. }), "{e:?}");
    let e = err("CALL nope('x')");
    assert!(matches!(e, SqlError::Unresolved { .. }));
    // Model-call functions type-check like any other function.
    let p = query("SELECT llm('summarise ' || name) AS s, llm_bool('ok?') AS b FROM nodes");
    assert_eq!(p.schema().fields[0].data_type, DataType::Text);
    assert_eq!(p.schema().fields[1].data_type, DataType::Bool);
    let p = query("SELECT e.item FROM nodes n CROSS JOIN LATERAL expand(n.name, 3) AS e");
    assert_eq!(names(&p), ["item"]);
}
