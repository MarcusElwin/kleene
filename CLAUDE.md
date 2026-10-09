# Kleene — conventions for contributors and coding agents

Read `docs/PLAN.md` first. It is the design; this file is the working rules.

## What this is

Rust workspace. The model writes CallSQL; we parse it (`kleene-sql`),
annotate it with call kinds (`kleene-algebra`), execute it asynchronously
(`kleene-exec`), route model calls (`kleene-llm`), run tools
(`kleene-tools`), persist tables, memo and trace in DuckDB
(`kleene-store`, `kleene-trace`), drive sessions (`kleene-harness`),
and show it all in a TUI over a daemon (`kleene-daemon`, `kleene-tui`).
Shared interface types live in `kleene-core`; change them deliberately,
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

The GitHub Actions workflow lives in `.github/workflows/ci.yml`: `fmt`,
`clippy, doc` and `test` run as three parallel jobs, each with rust-cache
keyed on the lockfile and sccache behind it, and a change that only touches
docs, the formula or the installer (`paths-ignore` in the workflow) runs
none of them.

## Commits and releases

Commit subjects follow Conventional Commits, because release-please reads
them: `feat(tui): fold a finished turn` bumps the minor version, `fix(llm):
…` the patch, `feat!:` or a `BREAKING CHANGE:` footer the minor while the
version is below 1.0, and `docs:`, `chore:`, `refactor:`, `test:`, `ci:`,
`bench:` move nothing. The scope is the crate or area without its `kleene-`
prefix. A merge commit's own subject is ignored; the commits inside count.

`release-please.yml` keeps one release pull request open on `main` from the
commits merged since the last tag. It edits the workspace `version` in
`Cargo.toml`, the thirteen `kleene-*` entries in `Cargo.lock` (the release
build is `--locked`) and `CHANGELOG.md`; `release-please-config.json` says
how and `.release-please-manifest.json` records the released version. Nobody
bumps the version by hand. Merging the release PR is the release button:
release-please tags the merge commit `v<version>` and creates the release
with the changelog as its notes, and that tag runs `release.yml`, which
builds the four targets and attaches the binaries and `SHA256SUMS`. After
the assets land, a follow-up PR pastes the new version and the four sha256
values from the workflow's summary into `Formula/kleene.rb`.

The `Cargo.lock` jsonpath compares `@.name.value` because release-please
parses TOML with a parser that wraps every scalar in a tagged object; plain
`@.name` matches nothing. Adding a crate to the workspace means adding its
name there.

A coding agent's GitHub App token cannot push `.github/workflows/`, so an
agent that changes the workflow must hand the push to a human. **Never
resolve a conflict on that path by deleting the file.** That happened once
(`1dbdeed`, merging m1 into m2) and the deletion rode up the whole stack, so
four milestone PRs silently ran no checks at all. If the push is rejected,
stop and say so.

DuckDB (`kleene-store`, feature `duckdb`) builds from source the first time:
about six minutes on four cores per variant. Two variants are normal and both
needed: `cargo clippy` (the check profile) and `cargo test` each run the
`libduckdb-sys` build script into their own `target/debug/build/libduckdb-sys-*/out`,
because cargo keys that directory on the compile mode. `cargo doc` shares
clippy's. Always use the `--workspace --all-features` shape above; `-p`,
`--exclude` or a different feature set makes a third.
`Cargo.toml` turns debug info off for `libduckdb-sys` only, so the
amalgamation's static library is ~500 MB per variant instead of over a
gigabyte and a test binary that links it is 200–300 MB. Keep `target/`
between runs; when disk runs low, delete `target/doc`,
`target/debug/incremental` and stale test binaries in `target/debug/deps`
before touching DuckDB artifacts. Every test binary that links the store
carries DuckDB, so crates that depend on it keep one integration-test binary
(`tests/all/main.rs` with `mod` files) and no examples.

[sccache](https://github.com/mozilla/sccache) makes a cold `target/` cheap:
with `RUSTC_WRAPPER=sccache` the `cc` crate routes DuckDB's C++ through it
too, so after one build every object is a cache hit whenever the same
variant is rebuilt from scratch (a wiped `target/`, a dependency bump that
misses rust-cache, a fresh container). It does not dedupe across the two
variants: the build script untars the sources into `OUT_DIR`, so the two
variants' preprocessed sources differ in their paths. CI sets it up; locally,
`cargo install sccache` and put

```toml
[build]
rustc-wrapper = "sccache"
```

in `~/.cargo/config.toml` (not the repo's: it would break every checkout
without sccache).

`DUCKDB_LIB_DIR` does **not** help while the dependency is declared
`features = ["bundled"]` — bundled compiles the amalgamation and ignores it.
Dropping `bundled` to link a prebuilt library is incompatible with the
`--all-features` shape above, which re-enables it; changing that is a
deliberate decision, not a CI tweak.

## Branches and PRs

Milestones are stacked PRs: `main` ← plan ← `m0-scaffold` ← `m1-relational-core`
← … Each milestone PR targets the branch below it. Work for a milestone happens
on sub-branches off that milestone's branch and merges into it.
