//! Run one case on both engines and compare.

use crate::compare::assert_same;
use crate::generator::{inserts, Case, TableSpec};
use callgebra_core::{
    standard_catalog, Batch, Catalog, Field, Row, Schema, TableDef, TableSource, Volatility,
};
use callgebra_exec::{execute_statement, ExecContext, MemorySink, StatementResult};
use callgebra_sql::{plan_sql, render_error, StatementKind};
use callgebra_store::DuckDbStore;
use std::sync::Arc;

fn schema_of(t: &TableSpec) -> Arc<Schema> {
    Arc::new(Schema::new(
        t.columns
            .iter()
            .map(|(n, ty)| Field::new(n.clone(), *ty))
            .collect(),
    ))
}

/// Catalog describing the case's tables plus the standard functions.
pub fn catalog_for(tables: &[TableSpec]) -> Catalog {
    let mut c = standard_catalog();
    for t in tables {
        c.add_table(TableDef {
            name: t.name.clone(),
            schema: (*schema_of(t)).clone(),
            source: TableSource::Stored,
            volatility: Volatility::Stable,
            description: String::new(),
        });
    }
    c
}

/// Run the case's SQL through the Callgebra planner and executor.
pub async fn run_callgebra(case: &Case) -> Result<Vec<Row>, String> {
    let catalog = catalog_for(&case.tables);
    let sink = Arc::new(MemorySink::new());
    for t in &case.tables {
        sink.load(&t.name, schema_of(t), t.rows.clone()).await;
    }
    let stmts = plan_sql(&case.sql, &catalog).map_err(|e| format!("plan: {}", render_error(&e)))?;
    let [stmt] = stmts.as_slice() else {
        return Err("expected exactly one statement".into());
    };
    if !matches!(stmt.kind, StatementKind::Query { .. }) {
        return Err("expected a query".into());
    }
    match execute_statement(stmt, ExecContext::new(sink))
        .await
        .map_err(|e| format!("exec: {e}"))?
    {
        StatementResult::Rows(b) => Ok(b.rows),
        other => Err(format!("unexpected result {other:?}")),
    }
}

/// Run the case's SQL on DuckDB.
pub async fn run_oracle(case: &Case) -> Result<Vec<Row>, String> {
    let db = DuckDbStore::in_memory().map_err(|e| e.to_string())?;
    for t in &case.tables {
        db.execute(&t.ddl()).await.map_err(|e| format!("oracle ddl: {e}"))?;
        for ins in inserts(t) {
            db.execute(&ins).await.map_err(|e| format!("oracle insert: {e}"))?;
        }
    }
    let b: Batch = db.query(&case.sql).await.map_err(|e| format!("oracle query: {e}"))?;
    Ok(b.rows)
}

/// Run both engines and compare. The error message contains the SQL, the
/// schema and both result sets, so a failing case can be reproduced by hand.
pub async fn run_case(case: &Case) -> Result<(), String> {
    let describe = || {
        let ddl: Vec<String> = case
            .tables
            .iter()
            .flat_map(|t| std::iter::once(t.ddl()).chain(inserts(t)))
            .collect();
        format!("-- schema --\n{}\n-- query --\n{}", ddl.join(";\n"), case.sql)
    };
    let expected = run_oracle(case).await.map_err(|e| format!("{e}\n{}", describe()))?;
    let actual = run_callgebra(case).await.map_err(|e| format!("{e}\n{}", describe()))?;
    assert_same(&expected, &actual, case.ordered).map_err(|e| format!("{e}\n{}", describe()))
}

/// Parse a corpus file: `-- ddl` block then `-- query` block, each SQL
/// statement terminated by `;`. Rows for the fixed corpus schema are inserted
/// through the DDL block, so the corpus is self-contained.
pub fn parse_corpus(text: &str) -> Result<Case, String> {
    let (ddl_part, query_part) = text
        .split_once("-- query")
        .ok_or("corpus file needs a '-- query' marker")?;
    let ddl: Vec<&str> = ddl_part
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with("--"))
        .collect();
    let mut tables: Vec<TableSpec> = vec![];
    for stmt in ddl {
        let upper = stmt.to_ascii_uppercase();
        if upper.starts_with("CREATE TABLE") {
            let name = stmt.split_whitespace().nth(2).ok_or("bad CREATE TABLE")?.to_string();
            let inner = stmt
                .split_once('(')
                .and_then(|(_, r)| r.rsplit_once(')'))
                .map(|(l, _)| l)
                .ok_or("bad column list")?;
            let columns = inner
                .split(',')
                .map(|c| {
                    let mut it = c.split_whitespace();
                    let n = it.next().unwrap_or("").to_string();
                    let ty = match it.next().unwrap_or("").to_ascii_uppercase().as_str() {
                        "BIGINT" | "INTEGER" | "INT" => callgebra_core::DataType::Int,
                        "DOUBLE" => callgebra_core::DataType::Float,
                        "BOOLEAN" => callgebra_core::DataType::Bool,
                        _ => callgebra_core::DataType::Text,
                    };
                    (n, ty)
                })
                .collect();
            tables.push(TableSpec {
                name,
                columns,
                rows: vec![],
            });
        } else if upper.starts_with("INSERT INTO") {
            let name = stmt.split_whitespace().nth(2).ok_or("bad INSERT")?.to_string();
            let t = tables
                .iter_mut()
                .find(|t| t.name.eq_ignore_ascii_case(&name))
                .ok_or_else(|| format!("insert into unknown table {name}"))?;
            let values = stmt.split_once("VALUES").ok_or("INSERT without VALUES")?.1;
            for row in values.split("),").map(|r| r.trim().trim_start_matches('(').trim_end_matches(')')) {
                let mut vals = vec![];
                for (i, cell) in split_cells(row).iter().enumerate() {
                    let cell = cell.trim();
                    let ty = t.columns.get(i).map(|c| c.1).ok_or("too many cells")?;
                    vals.push(if cell.eq_ignore_ascii_case("NULL") {
                        callgebra_core::Value::Null
                    } else {
                        match ty {
                            callgebra_core::DataType::Int => callgebra_core::Value::Int(cell.parse().map_err(|_| format!("bad int {cell}"))?),
                            callgebra_core::DataType::Float => callgebra_core::Value::Float(cell.parse().map_err(|_| format!("bad float {cell}"))?),
                            callgebra_core::DataType::Bool => callgebra_core::Value::Bool(cell.eq_ignore_ascii_case("true")),
                            _ => callgebra_core::Value::Text(cell.trim_matches('\'').replace("''", "'")),
                        }
                    });
                }
                t.rows.push(vals);
            }
        } else {
            return Err(format!("unsupported corpus statement: {stmt}"));
        }
    }
    let sql = query_part.trim().trim_end_matches(';').trim().to_string();
    let ordered = sql.to_ascii_uppercase().contains("ORDER BY");
    Ok(Case {
        tables,
        sql,
        ordered,
    })
}

fn split_cells(row: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut in_str = false;
    for ch in row.chars() {
        match ch {
            '\'' => {
                in_str = !in_str;
                cur.push(ch);
            }
            ',' if !in_str => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(ch),
        }
    }
    out.push(cur);
    out
}
