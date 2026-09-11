# Callgebra — conventions for contributors and coding agents

Read `docs/PLAN.md` first. It is the design; this file is the working rules.

## What this is

Rust workspace. The model writes CallSQL; we parse it (`callgebra-sql`),
annotate it with call kinds (`callgebra-algebra`), execute it asynchronously
(`callgebra-exec`), route model calls (`callgebra-llm`), run tools
(`callgebra-tools`), persist tables, memo and trace in DuckDB
(`callgebra-store`, `callgebra-trace`), drive sessions (`callgebra-harness`),
and show it all in a TUI over a daemon (`callgebra-daemon`, `callgebra-tui`).
Shared interface types live in `callgebra-core`; change them deliberately,
every crate depends on them.

## Rules

- Interfaces first. If a change needs a new shared type, add it to the
  interface crate in its own commit before implementing against it.
- Every crate compiles with `RUSTFLAGS="-D warnings"` under
  `cargo clippy --workspace --all-targets --all-features`, and
  `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"`.
  `missing_docs` is a warning, so every public item has a doc comment.
- `#![forbid(unsafe_code)]` in every crate.
- Errors are typed (`thiserror`) per crate; `anyhow` only in the binary.
- Anything the model reads (error text, rendered tables, EXPLAIN output) is
  part of the interface: keep it stable and tested.
- Volatility matters: a tool or function is `IMMUTABLE`, `STABLE` or
  `VOLATILE`, and the planner trusts that label. Never mark a side effect
  anything but `VOLATILE`.
- Tests that need a model use `ReplayProvider` fixtures under `tests/fixtures`;
  the relational core is tested differentially against DuckDB and needs no
  model.
- No SDKs for model providers. Adapters speak the wire format over `reqwest`.

## Commands

```bash
cargo fmt --all
RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets --all-features
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo run -- --help
```

The GitHub Actions workflow is staged in `ci/github-ci.yml` because the
agent's token cannot write `.github/workflows/`; a human moves it into place
(see `ci/README.md`).

DuckDB (`callgebra-store`, feature `duckdb`) builds from source the first time:
about ten minutes on four cores. Set `DUCKDB_LIB_DIR` to a prebuilt library to
skip it locally; CI caches it.

## Branches and PRs

Milestones are stacked PRs: `main` ← plan ← `m0-scaffold` ← `m1-relational-core`
← … Each milestone PR targets the branch below it. Work for a milestone happens
on sub-branches off that milestone's branch and merges into it.
