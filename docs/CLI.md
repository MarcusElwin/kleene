# The `callgebra` CLI

Install, point it at a model, and run. Every command below is one binary,
`callgebra`; run `callgebra --help` or `callgebra <command> --help` for the
flags as compiled.

- [Install](#install)
- [Set up a model provider](#configuring-a-model-provider)
- [First run](#first-run)
- [Global flags and files](#global-flags-and-files)
- [Commands](#commands): `run`, `resume`, `repl`, `explain`, `trace`, `tui`,
  `attach`, `daemon`, `learn`, `bench`
- [Working from source](#working-from-source)
- [Troubleshooting](#troubleshooting)

## Install

Prebuilt binaries for macOS (Apple silicon and Intel) and Linux (x86_64 and
aarch64) are attached to every tagged release. Pick one of the three.

### curl

```bash
curl -fsSL https://raw.githubusercontent.com/MarcusElwin/callgebra/main/install.sh | sh
```

The script detects your OS and architecture, downloads the latest release
tarball and its `.sha256`, verifies the checksum, and installs to
`~/.local/bin` (or `/usr/local/bin` when run as root). It tells you if the
destination is not on your `PATH`. Environment variables it honours:

| Variable | Effect | Default |
|---|---|---|
| `CALLGEBRA_VERSION` | Install a specific tag, e.g. `v0.1.0` | latest release |
| `CALLGEBRA_INSTALL` | Destination directory | `~/.local/bin` |
| `CALLGEBRA_REPO` | `owner/repo` to fetch from | `MarcusElwin/callgebra` |

Read it before piping it into a shell if that is your habit:
[`install.sh`](../install.sh) is sixty lines of POSIX `sh` and needs only
`curl` and `tar`.

### Homebrew

```bash
brew install MarcusElwin/callgebra/callgebra
```

This installs from the tap `MarcusElwin/homebrew-callgebra`, whose formula is
the template in [`Formula/callgebra.rb`](../Formula/callgebra.rb). Until the
tap exists, `brew install --formula ./Formula/callgebra.rb` from a checkout
works once the release checksums are filled in.

### cargo

```bash
cargo install --git https://github.com/MarcusElwin/callgebra callgebra
```

Needs Rust 1.88 or newer and about ten minutes: DuckDB is compiled from source
on the first build. `rustup` picks the toolchain pinned in
`rust-toolchain.toml` automatically inside a checkout; `cargo install --git`
uses your default toolchain.

### Check

```bash
callgebra --version
callgebra repl -c "SELECT 1 + 1 AS two"
```

The second line runs the relational core without a model and prints a
one-row table. If it prints `two` and `2`, the engine works.

## Configuring a model provider

Nothing that touches a model runs until one credential is set. Callgebra
speaks two wire formats directly, with no SDK and no gateway required.

**Anthropic**

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

`ANTHROPIC_AUTH_TOKEN` is accepted instead of the key, and
`ANTHROPIC_BASE_URL` overrides the endpoint. With only Anthropic configured
the default routing is: `root` on Opus 5 at high effort, `worker` and `judge`
on Sonnet 5, `proxy` on Haiku 4.5 at low effort, prompt prefix cached
everywhere, priced from the first-party rate card.

**OpenAI or any OpenAI-compatible endpoint**

```bash
export OPENAI_API_KEY=sk-...
export OPENAI_MODEL=gpt-5.4-mini          # optional; the model every alias resolves to
```

A local server needs only a URL: `OPENAI_BASE_URL=http://localhost:11434/v1`
with no key. With only this configured, every alias resolves to
`OPENAI_MODEL` (default `gpt-5.4-mini`) and nothing is priced, so dollar
budgets and estimates read zero.

**Both, or your own routing**

Write a router file and point at it:

```bash
export CALLGEBRA_ROUTER_TOML=~/.config/callgebra/router.toml
```

```toml
[aliases.root]
candidates = [{ provider = "anthropic", model = "claude-opus-5" }]
options = { effort = "high", cache_prefix = true }

[aliases.worker]
candidates = [
  { provider = "anthropic", model = "claude-sonnet-5" },
  { provider = "openai_compat", model = "gpt-5.4-mini" },   # failover
]
options = { effort = "low" }

[aliases.proxy]
candidates = [{ provider = "anthropic", model = "claude-haiku-4-5" }]

[aliases.judge]
candidates = [{ provider = "anthropic", model = "claude-sonnet-5" }]

[pricing."claude-opus-5"]
input_per_mtok = 5.0
output_per_mtok = 25.0
cache_read_per_mtok = 0.5
cache_write_per_mtok = 6.25
```

Candidates are tried in order; one whose calls keep failing is skipped until
its circuit breaker cools down. `options` takes `effort`, `cache_prefix`,
`temperature` and provider-specific `extra`. Aliases are what `SET
model.default = 'worker'`, `CREATE FUNCTION … MODEL 'proxy'` and `CREATE
AGENT … MODEL 'worker'` refer to; you can add your own names.

Every model call is memoised in the store by model and prompt fingerprint,
so re-running a task, demo or benchmark against the same database costs
nothing for prompts it has already seen.

## First run

```bash
mkdir demo && cd demo
export ANTHROPIC_API_KEY=sk-ant-...

# A task over a context file. The model gets ctx(ordinal, text), one row per
# paragraph, and writes CallSQL until FINAL.
callgebra run "Which project consumed the most hours in total?" \
  --context /path/to/callgebra/demos/oolong/corpus.txt --budget-calls 60

# See what it cost and what it did.
callgebra trace "SELECT depth, role, outcome, turns, calls, dollars FROM trace_sessions"
callgebra trace "SELECT sql, rows, calls FROM trace_statements ORDER BY started_at"

# Watch the next one live.
callgebra tui --run @task.txt --context corpus.txt
```

Everything lands in `.callgebra/run.duckdb` in the current directory. Delete
the directory to start clean, or pass `--db` to use another file.

## Global flags and files

| Flag | Meaning | Default |
|---|---|---|
| `--db <path>` | DuckDB file holding session tables, memo, trace and learning state | `.callgebra/run.duckdb` |
| `--workspace <dir>` | Root the tools (`files`, `grep`, `read`, `shell`, `write_file`, …) are confined to | current directory |
| `--log <filter>` | Log filter, e.g. `info` or `callgebra=debug` | `warn` |

Files under the working directory:

| Path | What |
|---|---|
| `.callgebra/run.duckdb` | the store: your tables, `memo`, `trace_*`, `callgebra_sessions`, learning and bench tables |
| `.callgebra/daemon.sock` | the daemon's Unix socket (`--socket` on `daemon`, `tui`, `attach`) |

`.callgebra/` is git-ignored in this repository; add it to yours.

Task arguments accept either literal text or `@path` to read a file.

## Commands

### `callgebra run <task>`

Run a task to completion: the model writes CallSQL turn by turn until
`FINAL`. Prints each turn's SQL and rendered result, then the final relation.

```
callgebra run <task|@file> [--context <file>] [--max-turns 30] [--max-depth 2]
              [--budget-calls N] [--budget-dollars X] [-q|--quiet]
```

| Flag | Meaning |
|---|---|
| `--context <file>` | Loaded as table `ctx(ordinal, text)`, one row per blank-line-separated paragraph |
| `--max-turns` | Turn cap for the root session (default 30); `resume` grants more |
| `--max-depth` | Deepest child session allowed (default 2); `0` disables `rlm` and `spawn` |
| `--budget-calls`, `--budget-dollars` | Budget for the whole run including children; a statement whose estimate exceeds what is left is refused with its plan |
| `-q` | Print only the final relation |

The run id is printed with the result and stored in `trace_runs`.

### `callgebra resume <run-id>`

Continue a root session that stopped at its turn cap or was interrupted.
Transcript, defined functions, agents, settings and spend are restored from
`callgebra_sessions`; turn numbering continues.

```
callgebra resume <run-id> [--max-turns 30] [-q]
callgebra trace "SELECT run, outcome, turns FROM trace_sessions WHERE depth = 0"
```

### `callgebra repl`

An interactive CallSQL REPL against the store. Reads statements from stdin,
one per line or terminated by `;`, plans each just before running it, and
prints the rendered rows with a footer of calls, tokens and dollars when a
statement made calls.

```bash
callgebra repl                                   # interactive
callgebra repl -c "SELECT 1 + 1 AS two"          # one statement
callgebra repl < demos/planner/three_way.sql     # a script
```

Everything in [`DIALECT.md`](DIALECT.md) works here: `CREATE FUNCTION … AS
PROMPT`, `CALL shell(...)`, `SET budget.calls = 20`, `EXPLAIN`, `WITH
RECURSIVE`. Tables you create persist in the store, so the next `repl` or
`run` sees them.

### `callgebra explain <sql>`

Print the call plan for one statement without executing it: per operator the
estimated rows, calls, tokens and dollars, call kinds, fences, the complexity
fragment, the rules that fired, the join-order plan space and the priced
alternatives. Needs no provider.

```bash
callgebra explain "SELECT c FROM candidates WHERE llm_bool('Is ' || c || ' a real place?')"
```

`EXPLAIN ANALYZE <statement>` inside `repl` also runs it and prints actuals.

### `callgebra trace <sql>`

Run DuckDB SQL directly over the store: the trace tables, the memo, the
learning and bench tables, and your own session tables.

```bash
callgebra trace "SELECT depth, role, outcome, turns, calls, dollars FROM trace_sessions ORDER BY started_at"
callgebra trace "SELECT alias, model, memo_hit, cost_usd FROM trace_calls ORDER BY started_at DESC LIMIT 20"
callgebra trace "SELECT tool, args, elapsed_ms FROM trace_tool_calls"
callgebra trace "SELECT cte, round, delta_rows FROM trace_rounds"
```

Tables: `trace_runs`, `trace_sessions`, `trace_statements`, `trace_calls`,
`trace_tool_calls`, `trace_rounds`, `trace_final`, `memo`,
`callgebra_sessions`, plus the learning tables (`tasks`, `attempts`,
`playbook`, `playbook_evals`, `task_ratings`, `solver_ratings`,
`generator_state`, view `trace_tasks`) and the bench tables (`evals`,
`bench_runs`). Your own session tables are here too.

### `callgebra tui`

Open the terminal UI over the engine daemon, starting one in the background
if nothing is listening on the socket.

```
callgebra tui [--run <task|@file>] [--context <file>] [--socket <path>]
```

Views and keys:

| Key | Action |
|---|---|
| `1` | session view: call tree with live status, transcript of the selected row, plan sidebar with memo-hit gauge |
| `2` | plan view: the selected statement's `EXPLAIN` with actuals |
| `3` | trace explorer: SQL over the store; `e` or `/` edits, `Enter` runs, `r` re-runs |
| `4` | task board for the continual loop (`r` refreshes; auto every two seconds) |
| `?` | help |
| `j` / `k`, arrows | move; `J` / `K`, PageUp/Down scroll the transcript |
| `f`, `Enter`, space | fold or unfold the selected node |
| `x`, `Esc` | cancel the selected statement, or the run from its root row |
| `d`, `q`, Ctrl-C | detach; the daemon and the run keep going |

Under 120×30 the layout switches to two columns.

### `callgebra attach`

Headless twin of `tui`: attaches to the daemon and prints every server
message as a JSON line. With `--run`, starts that task on connect and exits
when it finishes, so it can drive scripts and evaluations.

```bash
callgebra attach --run "Sum 1..4" | jq -c 'select(.msg == "run_finished")'
```

### `callgebra daemon`

Run the engine in the foreground on a Unix socket (`--socket`, default
`.callgebra/daemon.sock`). Clients speak newline-delimited JSON: `Subscribe`
with a cursor for replay, `StartRun`, `Submit` (REPL statements in a
client-owned session), `Query` (SQL over the store), `Cancel`, `CancelRun`,
`ListRuns`, `Detach`. Two clients see the same event stream; a client that
reconnects resumes from its last cursor. See
[`ARCHITECTURE.md`](ARCHITECTURE.md#processes).

### `callgebra learn`

The continual loop: generated and user tasks with code oracles, ratings, a
curriculum, and a playbook of winning SQL adopted only after a replay eval.
State lives in the store, so every command resumes where the last stopped.

| Command | What it does |
|---|---|
| `learn run [--tasks 10] [--budget-dollars X] [--minutes M] [--generators a,b] [--queue 3] [--replay 3]` | Run unattended until a limit: keep `--queue` pending tasks per generator, pick the task nearest even odds, attempt it, judge, update ratings and dials, gate playbook candidates over `--replay` recent tasks (`0` adopts outright) |
| `learn add <task|@file> [--kind user] [--context <file>] [--expect "a\|b;c\|d"] [--check '<shell>']` | Add a task. `--expect` gives exact rows; `--check` runs a shell oracle with the answer rows as JSON on stdin (exit 0 = pass); neither means it waits for human review |
| `learn generate <generator> [--count 3]` | Generate tasks at the generator's current dial: `sat3`, `graph`, `puzzle`, `corpus`, `repo`, `statements`, `contracts` |
| `learn propose <topic>` | Ask the model for a task, rubric and reference; a separate `judge` call grades attempts |
| `learn step [<task-id>]` | Attempt one task now, by id or the curriculum's pick |
| `learn board` | Counts per generator and status, and each dial |
| `learn report` | Solve rate, calls and depth by generator and difficulty (`trace_tasks`) |
| `learn playbook` | The version ledger with eval notes and wins/tries |
| `learn revert <version>` | Withdraw a playbook version |

```bash
callgebra learn run --tasks 50 --budget-dollars 5 --generators puzzle,corpus,graph   # overnight
callgebra learn report                                                                 # the morning query
```

### `callgebra bench`

Benchmarks over task packs (`tasks/<pack>/pack.json`) in one of three modes,
every task in a fresh workspace, every result a row in `evals`.

| Command | What it does |
|---|---|
| `bench run <pack-dir> [--mode learning\|frozen\|plain] [--limit N] [--record <dir>] [--replay <dir>]` | Run a pack. `learning` shows and adopts the playbook; `frozen` is the control; `plain` is a tool-calling agent on the same provider, tools and budget. `--record` saves every model reply as fixtures; `--replay` serves them offline |
| `bench build <out-dir> --from <generator> [--count 20] [--dial 0.5] [--seed 1]` | Freeze generator output into a pack; same seeds, same tasks |
| `bench terminal <out-dir>` | Write the built-in Terminal-Bench-style pack |
| `bench import-lab <lab-root> <out-dir>` | Import a Harvey LAB checkout into a pack |
| `bench report` | Accuracy, calls and dollars per pack and mode |
| `bench curve <run-id> [--window 5]` | The learning curve of one run as a sparkline and rolling mean |
| `bench csv` | Every eval row as CSV on stdout |

```bash
callgebra bench run tasks/oolong-like --mode frozen --record fixtures/oolong
callgebra bench run tasks/oolong-like --mode learning --replay fixtures/oolong
callgebra bench run tasks/terminal --mode plain
callgebra bench report && callgebra bench csv > evals.csv
```

Shipped packs are described in [`tasks/README.md`](../tasks/README.md).

## Working from source

```bash
git clone https://github.com/MarcusElwin/callgebra && cd callgebra
cargo build --release                       # first build compiles DuckDB: ~10 min, ~4 GB
./target/release/callgebra --help
cargo run -- repl -c "SELECT 42 AS answer"   # debug build, same engine
```

The checks CI runs, in the shape that reuses the DuckDB build:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

`CLAUDE.md` explains why the flags are shaped that way and how to keep
`target/` from filling the disk. Releases are built by the workflow in
`ci/release.yml` on `v*` tags (four targets, tarballs with SHA-256 files,
formula values printed in the job log).

## Troubleshooting

**`no model provider configured`** — set `ANTHROPIC_API_KEY`,
`ANTHROPIC_AUTH_TOKEN`, `OPENAI_API_KEY` or `OPENAI_BASE_URL`. `repl`,
`explain` and `trace` work without one as long as the statement makes no
calls.

**Estimates and dollars are all zero** — the model is not in the pricing
table. Only the Anthropic defaults come priced; add a `[pricing."model"]`
section to your router TOML.

**A statement is refused with its plan** — the estimate exceeded the
remaining call or dollar budget. Raise `--budget-*`, or narrow the query;
`EXPLAIN` shows where the calls go.

**The TUI shows nothing** — a stale socket from a killed daemon. Remove
`.callgebra/daemon.sock` or pass a fresh `--socket`.

**A `ctx` table already exists** — an older run left one in this database.
Current versions replace it per run; on an old file, `callgebra trace "DROP
TABLE ctx"`.

**Building takes forever or fills the disk** — that is DuckDB compiling from
source, once per build profile. Keep `target/` between runs, and see
`CLAUDE.md` before deleting anything under it.
