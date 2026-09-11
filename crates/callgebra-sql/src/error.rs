//! Frontend errors. Every variant can carry a hint, because the error text is
//! what the model reads next.

use thiserror::Error;

/// A parse, validation or resolution failure.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum SqlError {
    /// The text is not valid SQL.
    #[error("parse error: {message}")]
    Parse {
        /// Parser message.
        message: String,
        /// Suggested fix, if known.
        hint: Option<String>,
    },
    /// Valid SQL, but outside the CallSQL subset.
    #[error("unsupported: {construct}")]
    Unsupported {
        /// What was used.
        construct: String,
        /// The nearest supported form.
        hint: String,
    },
    /// A table, column or function name did not resolve.
    #[error("unknown {what}: {name}")]
    Unresolved {
        /// `table`, `column` or `function`.
        what: &'static str,
        /// The name as written.
        name: String,
        /// Similar names in the catalog.
        hint: Option<String>,
    },
    /// An expression has the wrong type for its context.
    #[error("type error: {message}")]
    Type {
        /// What went wrong.
        message: String,
    },
    /// The statement touches something the current role may not use.
    #[error("not permitted for this role: {name}")]
    NotPermitted {
        /// The relation or function.
        name: String,
    },
}

impl SqlError {
    /// The hint, if any, for rendering to the model.
    pub fn hint(&self) -> Option<&str> {
        match self {
            SqlError::Parse { hint, .. } => hint.as_deref(),
            SqlError::Unsupported { hint, .. } => Some(hint),
            SqlError::Unresolved { hint, .. } => hint.as_deref(),
            SqlError::Type { .. } | SqlError::NotPermitted { .. } => None,
        }
    }
}
