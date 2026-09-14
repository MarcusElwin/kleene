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
- Every crate is clean under
  `cargo clippy --workspace --all-targets --all-features -- -D warnings` and
  `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"`.
  Pass `-D warnings` after `--`, never through `RUSTFLAGS`: an environment
  flag changes cargo's fingerprint for every dependency and rebuilds DuckDB
  (ten minutes, four gigabytes) per distinct flag set.
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
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo run -- --help
```

The GitHub Actions workflow lives in `.github/workflows/ci.yml`. The copy
under `ci/` is a leftover from when it was staged there and is scheduled for
deletion; do not edit it and do not treat it as the source of truth.

A coding agent's GitHub App token cannot push `.github/workflows/`, so an
agent that changes the workflow must hand the push to a human. **Never
resolve a conflict on that path by deleting the file.** That happened once
(`1dbdeed`, merging m1 into m2) and the deletion rode up the whole stack, so
four milestone PRs silently ran no checks at all. If the push is rejected,
stop and say so.

DuckDB (`callgebra-store`, feature `duckdb`) builds from source the first time:
about ten minutes on four cores and four gigabytes per variant. Three
variants are normal and all needed: the `check` profile (clippy) has its own
`target/debug/build/libduckdb-sys-*` output, and `cargo build` and
`cargo test` link different `liblibduckdb_sys-*.rlib`s because dev-dependencies
change feature unification. Always use the `--workspace --all-features`
shape above; `-p`, `--exclude` or a different feature set makes a fourth.
Keep `target/` between runs; when disk runs low, delete `target/doc`,
`target/debug/incremental` and stale test binaries in `target/debug/deps`
before touching DuckDB artifacts.
`DUCKDB_LIB_DIR` does **not** help while the dependency is declared
`features = ["bundled"]` — bundled compiles the amalgamation and ignores it.
Dropping `bundled` to link a prebuilt library is incompatible with the
`--all-features` shape above, which re-enables it; changing that is a
deliberate decision, not a CI tweak.

## Branches and PRs

Milestones are stacked PRs: `main` ← plan ← `m0-scaffold` ← `m1-relational-core`
← … Each milestone PR targets the branch below it. Work for a milestone happens
on sub-branches off that milestone's branch and merges into it.
