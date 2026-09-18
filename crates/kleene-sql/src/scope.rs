//! Name-resolution scopes.
//!
//! A [`Scope`] is the list of columns visible to expressions at one point in a
//! query, in the position order of the operator's input schema. Scopes chain
//! to the enclosing query so correlated subqueries resolve outer columns.

use crate::error::SqlError;
use kleene_core::{DataType, Field, Schema};
use std::sync::Arc;

/// One visible column.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScopeItem {
    /// Table alias or name the column belongs to, if any.
    pub qualifier: Option<String>,
    /// Column name.
    pub name: String,
    /// Column type.
    pub data_type: DataType,
}

/// Where a name resolved to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Resolved {
    /// 0 for the current scope, 1 for the enclosing query, and so on.
    pub depth: usize,
    /// Position in that scope's input schema.
    pub index: usize,
    /// Column type.
    pub data_type: DataType,
}

/// Visible columns plus the chain of enclosing scopes.
#[derive(Debug, Clone)]
pub(crate) struct Scope<'a> {
    pub items: Vec<ScopeItem>,
    pub parent: Option<&'a Scope<'a>>,
}

impl<'a> Scope<'a> {
    /// A scope with no columns (e.g. `SELECT 1` without `FROM`).
    pub fn empty(parent: Option<&'a Scope<'a>>) -> Self {
        Self {
            items: vec![],
            parent,
        }
    }

    /// Concatenate two scopes (join output: left then right).
    pub fn concat(left: &Scope<'a>, right: &Scope<'a>, parent: Option<&'a Scope<'a>>) -> Self {
        let mut items = left.items.clone();
        items.extend(right.items.iter().cloned());
        Self { items, parent }
    }

    /// The schema these items describe.
    pub fn schema(&self) -> Arc<Schema> {
        Arc::new(Schema::new(
            self.items
                .iter()
                .map(|i| Field::new(i.name.clone(), i.data_type))
                .collect(),
        ))
    }

    /// Resolve `[qualifier.]name`, searching outward through enclosing scopes.
    pub fn resolve(&self, qualifier: Option<&str>, name: &str) -> Result<Resolved, SqlError> {
        let mut depth = 0;
        let mut scope: Option<&Scope<'a>> = Some(self);
        while let Some(s) = scope {
            let matches: Vec<(usize, &ScopeItem)> = s
                .items
                .iter()
                .enumerate()
                .filter(|(_, it)| {
                    it.name.eq_ignore_ascii_case(name)
                        && qualifier.is_none_or(|q| {
                            it.qualifier
                                .as_deref()
                                .is_some_and(|iq| iq.eq_ignore_ascii_case(q))
                        })
                })
                .collect();
            match matches.len() {
                0 => {}
                1 => {
                    return Ok(Resolved {
                        depth,
                        index: matches[0].0,
                        data_type: matches[0].1.data_type,
                    })
                }
                _ => {
                    let quals: Vec<String> = matches
                        .iter()
                        .map(|(_, it)| {
                            format!(
                                "{}.{}",
                                it.qualifier.clone().unwrap_or_else(|| "?".into()),
                                it.name
                            )
                        })
                        .collect();
                    return Err(SqlError::Unresolved {
                        what: "column",
                        name: name.to_string(),
                        hint: Some(format!(
                            "ambiguous; qualify it as one of {}",
                            quals.join(", ")
                        )),
                    });
                }
            }
            scope = s.parent;
            depth += 1;
        }
        let all: Vec<&str> = self.all_names();
        Err(SqlError::Unresolved {
            what: "column",
            name: match qualifier {
                Some(q) => format!("{q}.{name}"),
                None => name.to_string(),
            },
            hint: crate::similar::suggest(name, all.into_iter()).or_else(|| {
                let visible: Vec<&str> = self.items.iter().map(|i| i.name.as_str()).collect();
                if visible.is_empty() {
                    None
                } else {
                    Some(format!("visible columns: {}", visible.join(", ")))
                }
            }),
        })
    }

    /// Items belonging to a qualifier, for `t.*`.
    pub fn qualified(&self, qualifier: &str) -> Vec<(usize, &ScopeItem)> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                it.qualifier
                    .as_deref()
                    .is_some_and(|q| q.eq_ignore_ascii_case(qualifier))
            })
            .collect()
    }

    fn all_names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = vec![];
        let mut scope: Option<&Scope<'a>> = Some(self);
        while let Some(s) = scope {
            v.extend(s.items.iter().map(|i| i.name.as_str()));
            scope = s.parent;
        }
        v
    }
}

/// CTE names visible to a query, innermost last.
#[derive(Debug, Clone, Default)]
pub(crate) struct CteEnv {
    entries: Vec<(String, Arc<Schema>)>,
}

impl CteEnv {
    /// Bind a name.
    pub fn with(&self, name: &str, schema: Arc<Schema>) -> Self {
        let mut e = self.clone();
        e.entries.push((name.to_ascii_lowercase(), schema));
        e
    }

    /// Look a name up (innermost binding wins).
    pub fn get(&self, name: &str) -> Option<Arc<Schema>> {
        let lower = name.to_ascii_lowercase();
        self.entries
            .iter()
            .rev()
            .find(|(n, _)| *n == lower)
            .map(|(_, s)| s.clone())
    }

    /// Bound names, for hints.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(n, _)| n.as_str())
    }
}
