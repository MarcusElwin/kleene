//! Error type shared by the interface crate.

use thiserror::Error;

/// Errors raised by core types themselves (schema mismatches, bad casts).
#[derive(Debug, Error, Clone, PartialEq)]
pub enum CoreError {
    /// A row did not match the batch schema.
    #[error("row has {actual} values but schema has {expected} fields")]
    ArityMismatch {
        /// Fields in the schema.
        expected: usize,
        /// Values in the offending row.
        actual: usize,
    },
    /// A value could not be converted to the requested type.
    #[error("cannot cast {from} to {to}")]
    Cast {
        /// Source type name.
        from: &'static str,
        /// Target type name.
        to: String,
    },
    /// A column name was not found in a schema.
    #[error("no column named {0}")]
    NoSuchColumn(String),
}
