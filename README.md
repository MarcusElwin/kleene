# callgebra

**Callgebra — relational algebra for recursive model calls.**

Write declarative SQL. Compile joins, recursion, predicates and aggregation
into an execution graph of language-model calls, recursive sub-sessions and
tool calls. Plan and cost that graph, run it, and query the trace with the
same SQL. Watch it happen in a terminal UI.

```sql
SELECT candidate
FROM possibilities
WHERE VERIFY(candidate)
  AND NOT EXISTS (
    SELECT 1 FROM counterexamples ce
    WHERE REFUTE(candidate, ce.text)
  );
```

`VERIFY` is one model call per distinct candidate. The `NOT EXISTS` is an
anti-semi-join that stops on the first refuting counterexample. `EXPLAIN`
tells you how many calls that is before you spend them.

## Install

```bash
# curl: detects OS and architecture, verifies the SHA-256, installs to ~/.local/bin
curl -fsSL https://raw.githubusercontent.com/MarcusElwin/callgebra/main/install.sh | sh

# Homebrew
brew install MarcusElwin/callgebra/callgebra

# From source (compiles DuckDB the first time, about ten minutes)
cargo install --git https://github.com/MarcusElwin/callgebra callgebra
```

Prebuilt binaries cover macOS (Apple silicon, Intel) and Linux (x86_64,
aarch64). `CALLGEBRA_VERSION=v0.1.0` pins the installer to a tag and
`CALLGEBRA_INSTALL=/usr/local/bin` changes the destination. While the
repository is private, the raw URL is not served; set `GITHUB_TOKEN` and fetch
the script through the API instead (see [`docs/CLI.md`](docs/CLI.md#curl)).
Then:

```bash
callgebra repl -c "SELECT 1 + 1 AS two"    # the engine, no model needed
```

Full install, provider and command reference: [`docs/CLI.md`](docs/CLI.md).

## Quickstart

Point it at a model (one of these is enough):

```bash
export ANTHROPIC_API_KEY=sk-ant-...        # Anthropic, routed root/worker/proxy/judge by default
export OPENAI_API_KEY=sk-...               # or any OpenAI-compatible endpoint (OPENAI_BASE_URL, OPENAI_MODEL)
export CALLGEBRA_ROUTER_TOML=router.toml   # or your own aliases, failover and pricing
```

Run a task over a context, then read the trace:

```bash
callgebra run @demos/oolong/task.txt --context demos/oolong/corpus.txt --budget-calls 60
callgebra trace "SELECT depth, role, outcome, turns, calls, dollars FROM trace_sessions ORDER BY started_at"
```

The model receives the context as a table `ctx(ordinal, text)` and writes
CallSQL turn by turn: it can `SELECT` over the context, define prompt
functions, call tools with `CALL`, delegate with `rlm(...)` and
`spawn(...)`, and finish with `FINAL`. Every model call is memoised, so
running the same task again is free.

Watch it live, or explore the plan space without spending anything:

```bash
callgebra tui --run @demos/oolong/task.txt --context demos/oolong/corpus.txt
callgebra explain "SELECT c FROM candidates WHERE llm_bool('Is ' || c || ' a real place?')"
callgebra repl < demos/planner/three_way.sql     # join ordering over call predicates
```

Keep it learning overnight, and measure it:

```bash
callgebra learn run --tasks 20 --generators puzzle,corpus   # resumable; state is tables in the store
callgebra learn report                                       # the morning query
callgebra bench run tasks/terminal --mode frozen             # a task pack, one mode
callgebra bench report                                       # accuracy and cost per pack and mode
```

Everything lands in `.callgebra/run.duckdb` under the current directory.

## Commands

| Command | Does |
|---|---|
| `callgebra run <task>` | drive a model through the SQL turn loop to `FINAL` |
| `callgebra resume <run-id>` | continue a run that hit its turn cap |
| `callgebra repl [-c SQL]` | CallSQL against the store, interactive or scripted |
| `callgebra explain <sql>` | the call plan and its cost, without executing |
| `callgebra trace <sql>` | DuckDB SQL over the trace, memo and session tables |
| `callgebra tui` | the terminal UI over the engine daemon: call tree, transcript, plan, trace explorer, task board |
| `callgebra attach` | the same, headless: every event as a JSON line |
| `callgebra daemon` | the engine as a server on a Unix socket |
| `callgebra learn …` | the continual loop: tasks, oracles, ratings, curriculum, playbook |
| `callgebra bench …` | task packs under learning, frozen and plain-agent modes |

Details and every flag: [`docs/CLI.md`](docs/CLI.md).

## How it works

```
model reply ─▶ callgebra-sql ─▶ callgebra-algebra ─▶ budget check ─▶ callgebra-exec ─▶ rendered rows
   CallSQL     parse, resolve     call kinds, cost,   refuse if over    operators, recursion,   back to the model
               to a LogicalPlan   rules, EXPLAIN                        calls via LiveSink
                                                                            │
                                    callgebra-llm  (Anthropic, OpenAI-compatible, router, replay)
                                    callgebra-tools (files, grep, read, shell, write_file, patch, git_*, web_*)
                                    child sessions  (rlm, spawn: same loop at depth + 1 with a role and a budget slice)
                                                                            │
                                    callgebra-store (DuckDB): tables · memo · trace_* · sessions · learning · evals
```

- **A planner for calls.** Predicates that call a model have a cost the
  optimizer can see: join order, conjunct order, semi-joins for `EXISTS`,
  cascades through cheap proxies and beam-limited recursion are rewrite
  rules with a cost model. `EXPLAIN` shows the estimate before the spend.
- **Budgets as semantics.** Calls, tokens, dollars and depth are dimensions
  of a budget that child sessions inherit and slice; a statement the
  remaining budget cannot pay for is refused with its plan.
- **Learning as tables.** What the harness learns (playbook SQL, ratings,
  generator dials, sampled selectivities) is rows in DuckDB: inspectable,
  replay-gated and revertible.

Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the crates, the path
of a statement, sessions, the store and the daemon; [`docs/DIALECT.md`](docs/DIALECT.md)
for the language; [`docs/PLAN.md`](docs/PLAN.md) for the design and its
rationale; [`docs/WRITEUP.md`](docs/WRITEUP.md) for what has been measured;
[`docs/RESEARCH.md`](docs/RESEARCH.md) for the sources. Demos are in
[`demos/`](demos/README.md) and task packs in [`tasks/`](tasks/README.md).

## Status

M0 through M7 of the plan are merged: the CallSQL frontend, executor and
DuckDB store checked differentially against DuckDB; model and prompt-defined
functions, `CALL` tools, memo, budgets and `EXPLAIN`; the session loop with
`rlm`, `spawn`, `CREATE AGENT` and resume; the daemon and TUI; the planner
(join ordering over call predicates, cascades, beam recursion, sampled
selectivity, budget refusal); the continual loop; and the benchmark runner
with shipped packs. Every number in the write-up comes from the deterministic
suite; the first real-model runs are one `bench run --record` away from being
replayable.

## Developing

```bash
git clone https://github.com/MarcusElwin/callgebra && cd callgebra
cargo build                                                          # DuckDB compiles once, ~10 min
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

`CLAUDE.md` holds the working rules (interfaces first, typed errors, the
flag shapes that keep DuckDB from rebuilding) and `ci/README.md` the release
workflow. Written in Rust, model-agnostic with no SDK and no gateway
required, MIT licensed.

The name is also used by an unrelated small JavaScript library
(`fluture-js/callgebra`).
