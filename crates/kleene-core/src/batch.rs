//! Rows and batches: what flows between operators.
//!
//! Batches are row-oriented in v1. The executor processes small batches so a
//! columnar layout would buy little until operators are vectorised; the type
//! is opaque enough to change later without touching the planner.

use crate::error::CoreError;
use crate::value::{Schema, Value};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// One tuple.
pub type Row = Vec<Value>;

/// A chunk of rows sharing a schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    /// Column layout of every row.
    pub schema: Arc<Schema>,
    /// The rows.
    pub rows: Vec<Row>,
}

impl Batch {
    /// An empty batch with the given schema.
    pub fn empty(schema: Arc<Schema>) -> Self {
        Self {
            schema,
            rows: Vec::new(),
        }
    }

    /// Build a batch, checking every row's arity against the schema.
    pub fn try_new(schema: Arc<Schema>, rows: Vec<Row>) -> Result<Self, CoreError> {
        for row in &rows {
            if row.len() != schema.len() {
                return Err(CoreError::ArityMismatch {
                    expected: schema.len(),
                    actual: row.len(),
                });
            }
        }
        Ok(Self { schema, rows })
    }

    /// Number of rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// `true` if there are no rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Values of one column, by index.
    pub fn column(&self, idx: usize) -> impl Iterator<Item = &Value> + '_ {
        self.rows.iter().map(move |r| &r[idx])
    }

    /// Render as a fixed-width text table, the form results take in prompts.
    /// At most `max_rows` rows are shown; a trailing line reports the rest.
    pub fn render_table(&self, max_rows: usize) -> String {
        let names = self.schema.names();
        let shown: Vec<Vec<String>> = self
            .rows
            .iter()
            .take(max_rows)
            .map(|r| r.iter().map(Value::render).collect())
            .collect();
        let mut widths: Vec<usize> = names.iter().map(|n| n.chars().count()).collect();
        for row in &shown {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.chars().count().min(60));
            }
        }
        let fmt_row = |cells: &[String]| -> String {
            cells
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let c: String = if c.chars().count() > 60 {
                        let mut t: String = c.chars().take(57).collect();
                        t.push_str("...");
                        t
                    } else {
                        c.clone()
                    };
                    format!("{:<w$}", c, w = widths[i])
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let mut out = String::new();
        let header: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        out.push_str(&fmt_row(&header));
        out.push('\n');
        out.push_str(
            &widths
                .iter()
                .map(|w| "-".repeat(*w))
                .collect::<Vec<_>>()
                .join("-+-"),
        );
        out.push('\n');
        for row in &shown {
            out.push_str(&fmt_row(row));
            out.push('\n');
        }
        if self.rows.len() > max_rows {
            out.push_str(&format!(
                "... {} more rows ({} total)\n",
                self.rows.len() - max_rows,
                self.rows.len()
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{DataType, Field};

    fn schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int),
            Field::new("name", DataType::Text),
        ]))
    }

    #[test]
    fn arity_is_checked() {
        let bad = Batch::try_new(schema(), vec![vec![Value::Int(1)]]);
        assert!(matches!(bad, Err(CoreError::ArityMismatch { .. })));
    }

    #[test]
    fn render_truncates_rows() {
        let rows = (0..5)
            .map(|i| vec![Value::Int(i), Value::from(format!("n{i}"))])
            .collect();
        let b = Batch::try_new(schema(), rows).unwrap();
        let s = b.render_table(2);
        assert!(s.starts_with("id | name"));
        assert!(s.contains("... 3 more rows (5 total)"));
    }
}
