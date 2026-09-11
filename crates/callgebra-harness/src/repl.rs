//! One CallSQL statement at a time against a store: the core of the REPL and,
//! from M3, of the model's turn loop.

use crate::sink::StoreSink;
use crate::RenderOptions;
use callgebra_core::Catalog;
use callgebra_exec::{execute_statement, ExecContext, StatementResult};
use callgebra_sql::{plan_sql, render_error};
use callgebra_store::DuckDbStore;
use std::sync::Arc;
use std::time::Instant;

/// Runs statements and renders results as the model (or a human) sees them.
pub struct Repl {
    sink: Arc<StoreSink>,
    render: RenderOptions,
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
    pub async fn submit(&self, sql: &str) -> Vec<Rendered> {
        let catalog = self.sink.catalog().read().await.clone();
        let stmts = match plan_sql(sql, &catalog) {
            Ok(s) => s,
            Err(e) => {
                return vec![Rendered {
                    text: render_error(&e),
                    is_error: true,
                    is_final: false,
                }]
            }
        };
        let mut out = vec![];
        for stmt in &stmts {
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
