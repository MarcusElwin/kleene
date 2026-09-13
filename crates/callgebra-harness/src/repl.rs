//! One CallSQL statement at a time against a store: the core of the REPL and,
//! from M3, of the model's turn loop.

use crate::sink::StoreSink;
use crate::RenderOptions;
use callgebra_core::Catalog;
use callgebra_exec::{execute_statement, ExecContext, StatementResult};
use callgebra_sql::{parse, plan, plan_sql, render_error, Statement};
use callgebra_store::DuckDbStore;
use std::sync::Arc;
use std::time::Instant;

/// Runs statements and renders results as the model (or a human) sees them.
pub struct Repl {
    sink: Arc<StoreSink>,
    render: RenderOptions,
}

enum Pending {
    Planned(Statement),
    Ast(Box<sqlparser::ast::Statement>, String),
}

fn error(text: String) -> Rendered {
    Rendered {
        text,
        is_error: true,
        is_final: false,
    }
}

/// The rendering of one statement's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// Text to show.
    pub text: String,
    /// The statement failed.
    pub is_error: bool,
    /// `FINAL` was reached.
    pub is_final: bool,
}

impl Repl {
    /// Open a REPL over a store with the standard catalog.
    pub async fn new(store: DuckDbStore) -> Result<Self, callgebra_store::StoreError> {
        let sink = StoreSink::new(store, callgebra_core::standard_catalog()).await?;
        Ok(Self {
            sink: Arc::new(sink),
            render: RenderOptions::default(),
        })
    }

    /// The live catalog.
    pub fn catalog(&self) -> Arc<tokio::sync::RwLock<Catalog>> {
        self.sink.catalog()
    }

    /// Rendering options.
    pub fn render_options_mut(&mut self) -> &mut RenderOptions {
        &mut self.render
    }

    /// Plan and execute every statement in `sql`, rendering each outcome.
    ///
    /// Statements are planned one at a time, immediately before they run, so
    /// a table created by one statement is visible to the next. Execution
    /// stops at the first error or at `FINAL`.
    pub async fn submit(&self, sql: &str) -> Vec<Rendered> {
        let trimmed = sql.trim();
        let is_final = trimmed
            .get(..5)
            .is_some_and(|h| h.eq_ignore_ascii_case("FINAL"));
        let stmts: Vec<Statement> = if is_final {
            let catalog = self.sink.catalog().read().await.clone();
            match plan_sql(sql, &catalog) {
                Ok(s) => s,
                Err(e) => return vec![error(render_error(&e))],
            }
        } else {
            let asts = match parse(sql) {
                Ok(a) => a,
                Err(e) => return vec![error(render_error(&e))],
            };
            let single = asts.len() == 1;
            let mut planned = Vec::with_capacity(asts.len());
            // Plan lazily: statements after a DDL statement need the updated
            // catalog, so only the first is planned here and the rest inside
            // the loop below.
            planned.push(Pending::Ast(
                Box::new(asts[0].clone()),
                if single {
                    trimmed.to_string()
                } else {
                    asts[0].to_string()
                },
            ));
            for a in asts.into_iter().skip(1) {
                planned.push(Pending::Ast(Box::new(a.clone()), a.to_string()));
            }
            return self.run_pending(planned).await;
        };
        self.run_pending(stmts.into_iter().map(Pending::Planned).collect())
            .await
    }

    async fn run_pending(&self, pending: Vec<Pending>) -> Vec<Rendered> {
        let mut out = vec![];
        for item in pending {
            let stmt = match item {
                Pending::Planned(s) => s,
                Pending::Ast(ast, text) => {
                    let catalog = self.sink.catalog().read().await.clone();
                    match plan(&ast, &catalog) {
                        Ok(mut s) => {
                            s.sql = text;
                            s
                        }
                        Err(e) => {
                            out.push(error(render_error(&e)));
                            break;
                        }
                    }
                }
            };
            let stmt = &stmt;

            let started = Instant::now();
            let ctx = ExecContext::new(self.sink.clone());
            let rendered = match execute_statement(stmt, ctx).await {
                Ok(StatementResult::Rows(b)) => Rendered {
                    text: format!(
                        "{}{} row{} in {:.1?}",
                        b.render_table(self.render.max_rows),
                        b.len(),
                        if b.len() == 1 { "" } else { "s" },
                        started.elapsed()
                    ),
                    is_error: false,
                    is_final: false,
                },
                Ok(StatementResult::Final(b)) => Rendered {
                    text: format!("FINAL\n{}", b.render_table(self.render.max_rows)),
                    is_error: false,
                    is_final: true,
                },
                Ok(StatementResult::Affected { message, .. }) => Rendered {
                    text: message,
                    is_error: false,
                    is_final: false,
                },
                Ok(StatementResult::Set { key, value }) => Rendered {
                    text: format!("set {key} = {value}"),
                    is_error: false,
                    is_final: false,
                },
                Err(e) => Rendered {
                    text: format!("error: {e}"),
                    is_error: true,
                    is_final: false,
                },
            };
            let stop = rendered.is_error || rendered.is_final;
            out.push(rendered);
            if stop {
                break;
            }
        }
        out
    }
}
