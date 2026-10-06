# Contributing

The working rules live in [`CLAUDE.md`](https://github.com/MarcusElwin/kleene/blob/main/CLAUDE.md) and apply to people and coding agents alike. This page is the short version.

## Build and test

```bash
git clone https://github.com/MarcusElwin/kleene && cd kleene
cargo build                                                          # DuckDB compiles once, ~10 min
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

Always use the `--workspace --all-features` shape. DuckDB builds from source once per compile mode (clippy and test are two), about six minutes on four cores each and ~500 MB of static library per variant; a different `-p` or feature set makes a third. Keep `target/` between runs. Pass `-D warnings` after `--`, never through `RUSTFLAGS`, which would change every dependency's fingerprint and rebuild DuckDB. With [sccache](https://github.com/mozilla/sccache) as `rustc-wrapper`, a cold `target/` is cheap because the C++ objects are cache hits.

## Rules

- **Interfaces first.** A change that needs a new shared type adds it to `kleene-core` in its own commit before implementing against it.
- **Clean under clippy and rustdoc** with warnings denied; `missing_docs` is on, so every public item has a doc comment.
- `#![forbid(unsafe_code)]` in every crate.
- **Typed errors** (`thiserror`) per crate; `anyhow` only in the binary.
- **Model-facing text is interface.** Error text, rendered tables and `EXPLAIN` output are read by the model: keep them stable and asserted verbatim in tests.
- **Volatility is trusted.** A tool or function is `IMMUTABLE`, `STABLE` or `VOLATILE` and the planner believes the label. A side effect is always `VOLATILE`.
- **No network in tests.** Anything that needs a model uses `ReplayProvider` fixtures; the relational core is tested differentially against DuckDB.
- **No SDKs for model providers.** Adapters speak the wire format over `reqwest`.

## CI

[`.github/workflows/ci.yml`](https://github.com/MarcusElwin/kleene/blob/main/.github/workflows/ci.yml) runs `fmt`, `clippy, doc` and `test` as three parallel jobs with rust-cache and sccache; a change that only touches docs, the formula or the installer runs none of them. [`release.yml`](https://github.com/MarcusElwin/kleene/blob/main/.github/workflows/release.yml) builds the four release targets and publishes a GitHub release with checksums when a merge to `main` bumps the workspace version, when a `v*` tag is pushed, or on manual dispatch.

## Branches and PRs

Work happens on a branch off `main` and lands by PR. Milestones were stacked PRs (`main` ← plan ← `m0-scaffold` ← `m1-relational-core` ← ...), each targeting the branch below it; a merged branch is never reused. A PR description says what a reader would see before and after the change, then how.
