//! Differential testing of the CallSQL engine against DuckDB.
//!
//! [`generator`] produces random schemas, data and queries inside the
//! intersection of CallSQL and DuckDB SQL; [`run`] plans and executes each
//! query with the Kleene engine and with DuckDB; [`compare`] checks the
//! results agree. DuckDB is the oracle for every semantic question.

#![forbid(unsafe_code)]

pub mod compare;
pub mod generator;
pub mod run;

pub use generator::{Case, TableSpec};
pub use run::run_case;
