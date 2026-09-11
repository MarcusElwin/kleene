//! Differential tests against DuckDB.

use callgebra_difftest::generator::case_strategy;
use callgebra_difftest::run::{parse_corpus, run_case, run_oracle};
use proptest::prelude::*;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, max_shrink_iters: 2000, .. ProptestConfig::default() })]

    /// Generated queries produce the same rows on both engines.
    #[test]
    fn engine_matches_duckdb(case in case_strategy()) {
        let res = rt().block_on(run_case(&case));
        prop_assert!(res.is_ok(), "{}", res.unwrap_err());
    }

    /// Generated queries are valid DuckDB and valid CallSQL (generator sanity).
    #[test]
    fn generator_output_is_valid_on_both_sides(case in case_strategy()) {
        let oracle = rt().block_on(run_oracle(&case));
        prop_assert!(oracle.is_ok(), "oracle rejected: {}\n{}", oracle.unwrap_err(), case.sql);
        let catalog = callgebra_difftest::run::catalog_for(&case.tables);
        let planned = callgebra_sql::plan_sql(&case.sql, &catalog);
        prop_assert!(planned.is_ok(), "planner rejected: {}\n{}", callgebra_sql::render_error(&planned.unwrap_err()), case.sql);
    }
}

#[test]
fn curated_corpus_matches_duckdb() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    let rt = rt();
    let mut failures = vec![];
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let case = parse_corpus(&text).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        if let Err(e) = rt.block_on(run_case(&case)) {
            failures.push(format!("{}:\n{e}", f.display()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
