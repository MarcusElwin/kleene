//! Random schemas, data and queries in the intersection of CallSQL and DuckDB.
//!
//! Queries are built as a small AST and printed to SQL text that both engines
//! accept with the same meaning. Everything is qualified by table alias and
//! every ORDER BY is total (all output columns, `NULLS FIRST`), so ordered
//! comparison is possible; queries without ORDER BY are compared as multisets.
//!
//! Deliberately excluded (documented differences or unsupported on one side):
//! `/` on two integers where DuckDB and CallSQL agree on DOUBLE but rounding
//! of long decimals can differ in text rendering (allowed only under
//! `ROUND(..., 6)` in projections); string comparison collation beyond ASCII;
//! `LIKE` with escape characters; `LATERAL` subqueries (CallSQL only supports
//! LATERAL table functions); integer overflow (values are kept small).

use kleene_core::{DataType, Row, Value};
use proptest::prelude::*;

/// A generated table.
#[derive(Debug, Clone, PartialEq)]
pub struct TableSpec {
    /// Name.
    pub name: String,
    /// Columns.
    pub columns: Vec<(String, DataType)>,
    /// Rows.
    pub rows: Vec<Row>,
}

impl TableSpec {
    /// `CREATE TABLE` text for DuckDB.
    pub fn ddl(&self) -> String {
        let cols: Vec<String> = self
            .columns
            .iter()
            .map(|(n, t)| {
                let ty = match t {
                    DataType::Bool => "BOOLEAN",
                    DataType::Int => "BIGINT",
                    DataType::Float => "DOUBLE",
                    _ => "VARCHAR",
                };
                format!("{n} {ty}")
            })
            .collect();
        format!("CREATE TABLE {} ({})", self.name, cols.join(", "))
    }
}

/// One test case.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    /// Tables the query reads.
    pub tables: Vec<TableSpec>,
    /// The query.
    pub sql: String,
    /// Whether the query has a total ORDER BY.
    pub ordered: bool,
}

const TEXT_POOL: &[&str] = &["a", "b", "c", "ab", "ba", "x_y", "", "A", "hello", "%"];

fn value_strategy(t: DataType) -> BoxedStrategy<Value> {
    let non_null: BoxedStrategy<Value> = match t {
        DataType::Bool => any::<bool>().prop_map(Value::Bool).boxed(),
        DataType::Int => (-100i64..=100).prop_map(Value::Int).boxed(),
        DataType::Float => (-1000i64..=1000)
            .prop_map(|n| Value::Float(n as f64 / 4.0))
            .boxed(),
        _ => proptest::sample::select(TEXT_POOL)
            .prop_map(|s| Value::Text(s.to_string()))
            .boxed(),
    };
    prop_oneof![6 => non_null, 1 => Just(Value::Null)].boxed()
}

fn column_type() -> impl Strategy<Value = DataType> {
    prop_oneof![
        3 => Just(DataType::Int),
        2 => Just(DataType::Float),
        2 => Just(DataType::Text),
        1 => Just(DataType::Bool),
    ]
}

/// A table with 1-4 typed columns and 0-12 rows.
fn table_strategy(index: usize) -> impl Strategy<Value = TableSpec> {
    prop::collection::vec(column_type(), 1..=4).prop_flat_map(move |types| {
        let columns: Vec<(String, DataType)> = types
            .iter()
            .enumerate()
            .map(|(i, t)| (format!("c{i}"), *t))
            .collect();
        let row = types.iter().map(|t| value_strategy(*t)).collect::<Vec<_>>();
        prop::collection::vec(row, 0..=12).prop_map(move |rows| TableSpec {
            name: format!("t{index}"),
            columns: columns.clone(),
            rows,
        })
    })
}

/// Scalar expressions over the columns of one aliased table.
#[derive(Debug, Clone)]
enum Scalar {
    Col(String, usize),
    Int(i64),
    Float(f64),
    Text(String),
    Arith(Box<Scalar>, &'static str, Box<Scalar>),
    Cmp(Box<Scalar>, &'static str, Box<Scalar>),
    And(Box<Scalar>, Box<Scalar>),
    Or(Box<Scalar>, Box<Scalar>),
    Not(Box<Scalar>),
    IsNull(Box<Scalar>, bool),
    Case(Box<Scalar>, Box<Scalar>, Box<Scalar>),
    Upper(Box<Scalar>),
    Length(Box<Scalar>),
    Coalesce(Box<Scalar>, Box<Scalar>),
    NullIf(Box<Scalar>, Box<Scalar>),
    Abs(Box<Scalar>),
    Greatest(Box<Scalar>, Box<Scalar>),
    Concat(Box<Scalar>, Box<Scalar>),
    Between(Box<Scalar>, Box<Scalar>, Box<Scalar>),
    InList(Box<Scalar>, Vec<Scalar>),
    Cast(Box<Scalar>, &'static str),
}

impl Scalar {
    fn sql(&self) -> String {
        match self {
            Scalar::Col(alias, i) => format!("{alias}.c{i}"),
            Scalar::Int(i) => {
                if *i < 0 {
                    format!("({i})")
                } else {
                    i.to_string()
                }
            }
            Scalar::Float(f) => {
                let s = format!("{f:?}");
                if *f < 0.0 {
                    format!("({s})")
                } else {
                    s
                }
            }
            Scalar::Text(s) => format!("'{}'", s.replace('\'', "''")),
            Scalar::Arith(a, op, b) => format!("({} {op} {})", a.sql(), b.sql()),
            Scalar::Cmp(a, op, b) => format!("({} {op} {})", a.sql(), b.sql()),
            Scalar::And(a, b) => format!("({} AND {})", a.sql(), b.sql()),
            Scalar::Or(a, b) => format!("({} OR {})", a.sql(), b.sql()),
            Scalar::Not(a) => format!("(NOT {})", a.sql()),
            Scalar::IsNull(a, neg) => {
                format!("({} IS {}NULL)", a.sql(), if *neg { "NOT " } else { "" })
            }
            Scalar::Case(c, t, e) => format!(
                "(CASE WHEN {} THEN {} ELSE {} END)",
                c.sql(),
                t.sql(),
                e.sql()
            ),
            Scalar::Upper(a) => format!("upper({})", a.sql()),
            Scalar::Length(a) => format!("length({})", a.sql()),
            Scalar::Coalesce(a, b) => format!("coalesce({}, {})", a.sql(), b.sql()),
            Scalar::NullIf(a, b) => format!("nullif({}, {})", a.sql(), b.sql()),
            Scalar::Abs(a) => format!("abs({})", a.sql()),
            Scalar::Greatest(a, b) => format!("greatest({}, {})", a.sql(), b.sql()),
            Scalar::Concat(a, b) => format!("({} || {})", a.sql(), b.sql()),
            Scalar::Between(x, lo, hi) => {
                format!("({} BETWEEN {} AND {})", x.sql(), lo.sql(), hi.sql())
            }
            Scalar::InList(x, list) => format!(
                "({} IN ({}))",
                x.sql(),
                list.iter().map(Scalar::sql).collect::<Vec<_>>().join(", ")
            ),
            Scalar::Cast(a, ty) => format!("CAST({} AS {ty})", a.sql()),
        }
    }
}

/// Columns of a given type in an aliased table.
fn cols_of<'a>(alias: &'a str, table: &'a TableSpec, t: DataType) -> Vec<Scalar> {
    table
        .columns
        .iter()
        .enumerate()
        .filter(|(_, (_, ty))| *ty == t)
        .map(|(i, _)| Scalar::Col(alias.to_string(), i))
        .collect()
}

fn int_expr(alias: &str, table: &TableSpec, depth: u32) -> BoxedStrategy<Scalar> {
    let cols = cols_of(alias, table, DataType::Int);
    let leaf = if cols.is_empty() {
        (-20i64..=20).prop_map(Scalar::Int).boxed()
    } else {
        prop_oneof![
            3 => proptest::sample::select(cols),
            1 => (-20i64..=20).prop_map(Scalar::Int),
        ]
        .boxed()
    };
    if depth == 0 {
        return leaf;
    }
    let a = alias.to_string();
    let tbl = table.clone();
    let inner = move || int_expr(&a, &tbl, depth - 1);
    prop_oneof![
        4 => leaf,
        1 => (inner(), proptest::sample::select(vec!["+", "-", "*"]), inner()).prop_map(|(x, op, y)| Scalar::Arith(Box::new(x), op, Box::new(y))),
        1 => inner().prop_map(|x| Scalar::Abs(Box::new(x))),
        1 => (inner(), inner()).prop_map(|(x, y)| Scalar::Coalesce(Box::new(x), Box::new(y))),
        1 => (inner(), inner()).prop_map(|(x, y)| Scalar::NullIf(Box::new(x), Box::new(y))),
        1 => (inner(), inner()).prop_map(|(x, y)| Scalar::Greatest(Box::new(x), Box::new(y))),
    ]
    .boxed()
}

fn float_expr(alias: &str, table: &TableSpec) -> BoxedStrategy<Scalar> {
    let cols = cols_of(alias, table, DataType::Float);
    let lit = (-40i64..=40)
        .prop_map(|n| Scalar::Float(n as f64 / 4.0))
        .boxed();
    let leaf = if cols.is_empty() {
        lit
    } else {
        prop_oneof![3 => proptest::sample::select(cols), 1 => lit].boxed()
    };
    let ints = int_expr(alias, table, 0);
    prop_oneof![
        3 => leaf.clone(),
        1 => (leaf.clone(), leaf.clone()).prop_map(|(x, y)| Scalar::Arith(Box::new(x), "+", Box::new(y))),
        1 => (ints.clone(), ints).prop_map(|(x, y)| Scalar::Arith(Box::new(x), "/", Box::new(y))),
        1 => (leaf.clone(), int_expr(alias, table, 0)).prop_map(|(x, y)| Scalar::Arith(Box::new(x), "*", Box::new(y))),
    ]
    .boxed()
}

fn text_expr(alias: &str, table: &TableSpec) -> BoxedStrategy<Scalar> {
    let cols = cols_of(alias, table, DataType::Text);
    let lit = proptest::sample::select(TEXT_POOL)
        .prop_map(|s| Scalar::Text(s.to_string()))
        .boxed();
    let leaf = if cols.is_empty() {
        lit
    } else {
        prop_oneof![3 => proptest::sample::select(cols), 1 => lit].boxed()
    };
    prop_oneof![
        3 => leaf.clone(),
        1 => leaf.clone().prop_map(|x| Scalar::Upper(Box::new(x))),
        1 => (leaf.clone(), leaf.clone()).prop_map(|(x, y)| Scalar::Concat(Box::new(x), Box::new(y))),
        1 => (leaf.clone(), leaf).prop_map(|(x, y)| Scalar::Coalesce(Box::new(x), Box::new(y))),
        1 => int_expr(alias, table, 0).prop_map(|x| Scalar::Cast(Box::new(x), "VARCHAR")),
    ]
    .boxed()
}

fn bool_expr(alias: &str, table: &TableSpec, depth: u32) -> BoxedStrategy<Scalar> {
    let bool_cols = cols_of(alias, table, DataType::Bool);
    let ints = int_expr(alias, table, 1);
    let floats = float_expr(alias, table);
    let texts = text_expr(alias, table);
    let cmp_ops = || proptest::sample::select(vec!["=", "<>", "<", "<=", ">", ">="]);
    let mut leaves: Vec<BoxedStrategy<Scalar>> = vec![
        (ints.clone(), cmp_ops(), ints.clone())
            .prop_map(|(a, op, b)| Scalar::Cmp(Box::new(a), op, Box::new(b)))
            .boxed(),
        (floats.clone(), cmp_ops(), ints.clone())
            .prop_map(|(a, op, b)| Scalar::Cmp(Box::new(a), op, Box::new(b)))
            .boxed(),
        (
            texts.clone(),
            proptest::sample::select(vec!["=", "<>"]),
            texts.clone(),
        )
            .prop_map(|(a, op, b)| Scalar::Cmp(Box::new(a), op, Box::new(b)))
            .boxed(),
        ints.clone()
            .prop_map(|a| Scalar::IsNull(Box::new(a), false))
            .boxed(),
        texts
            .clone()
            .prop_map(|a| Scalar::IsNull(Box::new(a), true))
            .boxed(),
        (ints.clone(), ints.clone(), ints.clone())
            .prop_map(|(x, lo, hi)| Scalar::Between(Box::new(x), Box::new(lo), Box::new(hi)))
            .boxed(),
        (
            ints.clone(),
            prop::collection::vec((-20i64..=20).prop_map(Scalar::Int), 1..=3),
        )
            .prop_map(|(x, list)| Scalar::InList(Box::new(x), list))
            .boxed(),
        (texts.clone(), texts.clone())
            .prop_map(|(a, b)| {
                Scalar::Cmp(
                    Box::new(Scalar::Length(Box::new(a))),
                    "<",
                    Box::new(Scalar::Length(Box::new(b))),
                )
            })
            .boxed(),
    ];
    if !bool_cols.is_empty() {
        leaves.push(proptest::sample::select(bool_cols).boxed());
    }
    let leaf = proptest::strategy::Union::new(leaves).boxed();
    if depth == 0 {
        return leaf;
    }
    let a = alias.to_string();
    let tbl = table.clone();
    let inner = move || bool_expr(&a, &tbl, depth - 1);
    prop_oneof![
        3 => leaf,
        1 => (inner(), inner()).prop_map(|(x, y)| Scalar::And(Box::new(x), Box::new(y))),
        1 => (inner(), inner()).prop_map(|(x, y)| Scalar::Or(Box::new(x), Box::new(y))),
        1 => inner().prop_map(|x| Scalar::Not(Box::new(x))),
    ]
    .boxed()
}

/// Any typed expression usable in a projection.
fn any_expr(alias: &str, table: &TableSpec) -> BoxedStrategy<Scalar> {
    prop_oneof![
        3 => int_expr(alias, table, 1),
        2 => float_expr(alias, table).prop_map(|f| Scalar::Arith(Box::new(Scalar::Int(0)), "+", Box::new(f))),
        2 => text_expr(alias, table),
        1 => bool_expr(alias, table, 0),
        1 => (bool_expr(alias, table, 0), int_expr(alias, table, 0), int_expr(alias, table, 0)).prop_map(|(c, t, e)| Scalar::Case(Box::new(c), Box::new(t), Box::new(e))),
    ]
    .boxed()
}

fn all_columns(alias: &str, table: &TableSpec) -> Vec<String> {
    (0..table.columns.len())
        .map(|i| format!("{alias}.c{i}"))
        .collect()
}

fn order_by_all(n_cols: usize) -> String {
    let keys: Vec<String> = (1..=n_cols).map(|i| format!("{i} NULLS FIRST")).collect();
    format!(" ORDER BY {}", keys.join(", "))
}

/// The query shapes.
#[derive(Debug, Clone)]
enum Shape {
    /// SELECT exprs FROM t0 a [WHERE] [ORDER BY all] [LIMIT]
    Simple {
        exprs: Vec<Scalar>,
        filter: Option<Scalar>,
        distinct: bool,
        limit: Option<(usize, usize)>,
    },
    /// SELECT ... FROM t0 a JOIN t1 b ON a.ci = b.cj [WHERE]
    Join {
        kind: &'static str,
        on: Option<(usize, usize)>,
        filter: Option<Scalar>,
        project_all: bool,
    },
    /// SELECT key, aggregates FROM t0 a [WHERE] GROUP BY key [HAVING]
    Group {
        key: usize,
        aggs: Vec<String>,
        filter: Option<Scalar>,
        having: Option<&'static str>,
    },
    /// SELECT exprs FROM t0 a UNION [ALL] SELECT exprs FROM t0 b
    Union { all: bool, col: usize },
    /// EXISTS / NOT EXISTS / IN subquery correlated to t1
    Sub {
        form: &'static str,
        outer_col: usize,
        inner_col: usize,
    },
    /// Scalar subquery MAX(t1.col) in the projection
    ScalarSub { col: usize },
    /// WITH c AS (SELECT ... FROM t0) SELECT ... FROM c
    With { col: usize, filter: Option<Scalar> },
    /// generate_series in FROM and LATERAL
    Series { lateral: bool, n: i64 },
    /// Transitive closure over an edge table (t0 with two BIGINT columns)
    Recursive { all: bool, bound: usize },
}

fn shape_strategy(tables: &[TableSpec]) -> BoxedStrategy<Shape> {
    let t0 = tables[0].clone();
    let t1 = tables.get(1).cloned().unwrap_or_else(|| t0.clone());
    let n0 = t0.columns.len();
    let n1 = t1.columns.len();
    let simple = (
        prop::collection::vec(any_expr("a", &t0), 1..=4),
        prop::option::of(bool_expr("a", &t0, 2)),
        any::<bool>(),
        prop::option::of((0usize..=3, 1usize..=6)),
    )
        .prop_map(|(exprs, filter, distinct, limit)| Shape::Simple {
            exprs,
            filter,
            distinct,
            limit,
        });
    // Join keys must have the same type on both sides; DuckDB refuses to
    // compare VARCHAR with BIGINT.
    let join_pairs: Vec<(usize, usize)> = (0..n0)
        .flat_map(|i| (0..n1).map(move |j| (i, j)))
        .filter(|(i, j)| t0.columns[*i].1 == t1.columns[*j].1)
        .collect();
    let join_kinds = if join_pairs.is_empty() {
        vec!["CROSS JOIN"]
    } else {
        vec!["JOIN", "LEFT JOIN", "CROSS JOIN"]
    };
    let join_pair_strategy = if join_pairs.is_empty() {
        Just((0usize, 0usize)).boxed()
    } else {
        proptest::sample::select(join_pairs).boxed()
    };
    let join = (
        proptest::sample::select(join_kinds),
        join_pair_strategy,
        prop::option::of(bool_expr("a", &t0, 1)),
        any::<bool>(),
    )
        .prop_map(|(kind, (i, j), filter, project_all)| Shape::Join {
            kind,
            on: if kind == "CROSS JOIN" {
                None
            } else {
                Some((i, j))
            },
            filter,
            project_all,
        });
    let int_cols0: Vec<usize> = t0
        .columns
        .iter()
        .enumerate()
        .filter(|(_, (_, t))| *t == DataType::Int)
        .map(|(i, _)| i)
        .collect();
    let group = (
        0..n0,
        prop::collection::vec(
            proptest::sample::select({
                let mut v = vec!["COUNT(*)".to_string()];
                for i in 0..n0 {
                    v.push(format!("COUNT(a.c{i})"));
                    v.push(format!("MIN(a.c{i})"));
                    v.push(format!("MAX(a.c{i})"));
                }
                for i in &int_cols0 {
                    v.push(format!("SUM(a.c{i})"));
                    v.push(format!("AVG(a.c{i})"));
                    v.push(format!("COUNT(DISTINCT a.c{i})"));
                }
                v
            }),
            1..=3,
        ),
        prop::option::of(bool_expr("a", &t0, 1)),
        prop::option::of(proptest::sample::select(vec![
            "COUNT(*) > 1",
            "COUNT(*) >= 1",
            "COUNT(*) < 3",
        ])),
    )
        .prop_map(|(key, aggs, filter, having)| Shape::Group {
            key,
            aggs,
            filter,
            having,
        });
    let union = (any::<bool>(), 0..n0).prop_map(|(all, col)| Shape::Union { all, col });
    let sub_pairs: Vec<(usize, usize)> = (0..n0)
        .flat_map(|i| (0..n1).map(move |j| (i, j)))
        .filter(|(i, j)| t0.columns[*i].1 == t1.columns[*j].1 && t0.columns[*i].1 != DataType::Bool)
        .collect();
    let mut shapes: Vec<BoxedStrategy<Shape>> =
        vec![simple.boxed(), join.boxed(), group.boxed(), union.boxed()];
    if !sub_pairs.is_empty() {
        let pairs = sub_pairs.clone();
        shapes.push(
            (
                proptest::sample::select(vec!["EXISTS", "NOT EXISTS", "IN", "NOT IN"]),
                proptest::sample::select(pairs),
            )
                .prop_map(|(form, (outer_col, inner_col))| Shape::Sub {
                    form,
                    outer_col,
                    inner_col,
                })
                .boxed(),
        );
    }
    let numeric1: Vec<usize> = t1
        .columns
        .iter()
        .enumerate()
        .filter(|(_, (_, t))| matches!(t, DataType::Int | DataType::Float))
        .map(|(i, _)| i)
        .collect();
    if !numeric1.is_empty() {
        shapes.push(
            proptest::sample::select(numeric1)
                .prop_map(|col| Shape::ScalarSub { col })
                .boxed(),
        );
    }
    shapes.push(
        (0..n0, prop::option::of(bool_expr("c", &t0, 1)))
            .prop_map(|(col, filter)| Shape::With { col, filter })
            .boxed(),
    );
    shapes.push(
        (any::<bool>(), 1i64..=4)
            .prop_map(|(lateral, n)| Shape::Series { lateral, n })
            .boxed(),
    );
    if int_cols0.len() >= 2 {
        shapes.push(
            (any::<bool>(), 1usize..=4)
                .prop_map(|(all, bound)| Shape::Recursive { all, bound })
                .boxed(),
        );
    }
    proptest::strategy::Union::new(shapes).boxed()
}

fn render(shape: &Shape, tables: &[TableSpec]) -> (String, bool) {
    let t0 = &tables[0];
    let t1 = tables.get(1).unwrap_or(t0);
    let n0 = t0.columns.len();
    match shape {
        Shape::Simple {
            exprs,
            filter,
            distinct,
            limit,
        } => {
            let cols: Vec<String> = exprs
                .iter()
                .enumerate()
                .map(|(i, e)| format!("{} AS e{i}", e.sql()))
                .collect();
            let mut sql = format!(
                "SELECT {}{} FROM {} a",
                if *distinct { "DISTINCT " } else { "" },
                cols.join(", "),
                t0.name
            );
            if let Some(f) = filter {
                sql += &format!(" WHERE {}", f.sql());
            }
            sql += &order_by_all(exprs.len());
            if let Some((off, lim)) = limit {
                sql += &format!(" LIMIT {lim} OFFSET {off}");
            }
            (sql, true)
        }
        Shape::Join {
            kind,
            on,
            filter,
            project_all,
        } => {
            let mut cols = all_columns("a", t0);
            if *project_all {
                cols.extend(all_columns("b", t1));
            }
            let mut sql = format!(
                "SELECT {} FROM {} a {kind} {} b",
                cols.join(", "),
                t0.name,
                t1.name
            );
            if let Some((i, j)) = on {
                sql += &format!(" ON a.c{i} = b.c{j}");
            }
            if let Some(f) = filter {
                sql += &format!(" WHERE {}", f.sql());
            }
            sql += &order_by_all(cols.len());
            (sql, true)
        }
        Shape::Group {
            key,
            aggs,
            filter,
            having,
        } => {
            let mut sql = format!(
                "SELECT a.c{key} AS k, {} FROM {} a",
                aggs.join(", "),
                t0.name
            );
            if let Some(f) = filter {
                sql += &format!(" WHERE {}", f.sql());
            }
            sql += &format!(" GROUP BY a.c{key}");
            if let Some(h) = having {
                sql += &format!(" HAVING {h}");
            }
            sql += &order_by_all(1 + aggs.len());
            (sql, true)
        }
        Shape::Union { all, col } => {
            let sql = format!(
                "SELECT a.c{col} AS v FROM {} a UNION {}SELECT b.c{col} AS v FROM {} b ORDER BY 1 NULLS FIRST",
                t0.name,
                if *all { "ALL " } else { "" },
                t0.name
            );
            (sql, true)
        }
        Shape::Sub {
            form,
            outer_col,
            inner_col,
        } => {
            let cols = all_columns("a", t0);
            let cond = match *form {
                "EXISTS" | "NOT EXISTS" => format!(
                    "{form} (SELECT 1 FROM {} b WHERE b.c{inner_col} = a.c{outer_col})",
                    t1.name
                ),
                _ => format!(
                    "a.c{outer_col} {form} (SELECT b.c{inner_col} FROM {} b)",
                    t1.name
                ),
            };
            let sql = format!(
                "SELECT {} FROM {} a WHERE {cond}{}",
                cols.join(", "),
                t0.name,
                order_by_all(n0)
            );
            (sql, true)
        }
        Shape::ScalarSub { col } => {
            let sql = format!(
                "SELECT a.c0, (SELECT MAX(b.c{col}) FROM {} b) AS m FROM {} a ORDER BY 1 NULLS FIRST, 2 NULLS FIRST",
                t1.name, t0.name
            );
            (sql, true)
        }
        Shape::With { col, filter } => {
            let mut sql = format!(
                "WITH c AS (SELECT a.c{col} AS c{col}, COUNT(*) AS n FROM {} a GROUP BY a.c{col}) SELECT c.c{col}, c.n FROM c",
                t0.name
            );
            if let Some(f) = filter {
                // The filter references c.c{col} only if it is the same column; keep it simple.
                let f_sql = f.sql();
                if f_sql.contains(&format!("c.c{col}"))
                    && !(0..n0)
                        .filter(|i| i != col)
                        .any(|i| f_sql.contains(&format!("c.c{i}")))
                {
                    sql += &format!(" WHERE {f_sql}");
                }
            }
            sql += " ORDER BY 1 NULLS FIRST, 2 NULLS FIRST";
            (sql, true)
        }
        Shape::Series { lateral, n } => {
            let sql = if *lateral {
                format!(
                    "SELECT a.c0, g.generate_series AS k FROM {} a CROSS JOIN LATERAL generate_series(1, {n}) AS g ORDER BY 1 NULLS FIRST, 2 NULLS FIRST",
                    t0.name
                )
            } else {
                format!("SELECT generate_series AS k FROM generate_series(1, {n}) ORDER BY 1 NULLS FIRST")
            };
            (sql, true)
        }
        Shape::Recursive { all, bound } => {
            let ints: Vec<usize> = t0
                .columns
                .iter()
                .enumerate()
                .filter(|(_, (_, t))| *t == DataType::Int)
                .map(|(i, _)| i)
                .collect();
            let (s, d) = (ints[0], ints[1]);
            let sql = if *all {
                format!(
                    "WITH RECURSIVE p(n, depth) AS (SELECT e.c{d}, 1 FROM {t} e WHERE e.c{s} IS NOT NULL UNION ALL SELECT e.c{d}, p.depth + 1 FROM p JOIN {t} e ON e.c{s} = p.n WHERE p.depth < {bound}) SELECT p.n, p.depth FROM p ORDER BY 1 NULLS FIRST, 2 NULLS FIRST",
                    t = t0.name
                )
            } else {
                format!(
                    "WITH RECURSIVE r(n) AS (SELECT e.c{d} FROM {t} e WHERE e.c{s} IS NOT NULL UNION SELECT e.c{d} FROM r JOIN {t} e ON e.c{s} = r.n) SELECT r.n FROM r ORDER BY 1 NULLS FIRST",
                    t = t0.name
                )
            };
            (sql, true)
        }
    }
}

/// A random case: 2 tables and one query.
pub fn case_strategy() -> impl Strategy<Value = Case> {
    (table_strategy(0), table_strategy(1)).prop_flat_map(|(t0, t1)| {
        let tables = vec![t0, t1];
        let shapes = shape_strategy(&tables);
        shapes.prop_map(move |shape| {
            let (sql, ordered) = render(&shape, &tables);
            Case {
                tables: tables.clone(),
                sql,
                ordered,
            }
        })
    })
}

/// `INSERT` statements for a table (DuckDB syntax; also valid CallSQL).
pub fn inserts(t: &TableSpec) -> Vec<String> {
    t.rows
        .chunks(50)
        .map(|chunk| {
            let vals: Vec<String> = chunk
                .iter()
                .map(|r| format!("({})", r.iter().map(literal).collect::<Vec<_>>().join(", ")))
                .collect();
            format!("INSERT INTO {} VALUES {}", t.name, vals.join(", "))
        })
        .collect()
}

/// SQL literal for a value.
pub fn literal(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => format!("{f:?}"),
        Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Json(j) => format!("'{}'", j.to_string().replace('\'', "''")),
        Value::Vector(v) => format!(
            "[{}]",
            v.iter()
                .map(|x| format!("{x:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
