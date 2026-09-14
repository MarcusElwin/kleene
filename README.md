# kleene

**Kleene — relational algebra for recursive model calls.**

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
brew install MarcusElwin/callgebra/kleene

# From source (compiles DuckDB the first time, about ten minutes)
cargo install --git https://github.com/MarcusElwin/callgebra kleene
```

Prebuilt binaries cover macOS (Apple silicon, Intel) and Linux (x86_64,
aarch64). `KLEENE_VERSION=v0.1.0` pins the installer to a tag and
`KLEENE_INSTALL=/usr/local/bin` changes the destination. While the
repository is private, the raw URL is not served; set `GITHUB_TOKEN` and fetch
the script through the API instead (see [`docs/CLI.md`](docs/CLI.md#curl)).
Then:

```bash
kleene repl -c "SELECT 1 + 1 AS two"    # the engine, no model needed
```

Full install, provider and command reference: [`docs/CLI.md`](docs/CLI.md).

## Quickstart

Point it at a model. The wizard asks which providers to use and stores the
keys owner-readable under `~/.config/kleene/`; environment variables win
over the file when both are set:

```bash
kleene setup                               # pick Anthropic, OpenAI, or a compatible endpoint (Ollama, vLLM, a gateway)
export ANTHROPIC_API_KEY=sk-ant-...        # or just the environment: Anthropic, routed root/worker/proxy/judge by default
export OPENAI_API_KEY=sk-...               # or any OpenAI-compatible endpoint (OPENAI_BASE_URL, OPENAI_MODEL)
```

Run a task over a context, then read the trace:

```bash
kleene run @demos/oolong/task.txt --context demos/oolong/corpus.txt --budget-calls 60
kleene trace "SELECT depth, role, outcome, turns, calls, dollars FROM trace_sessions ORDER BY started_at"
```

The model receives the context as a table `ctx(ordinal, text)` and writes
CallSQL turn by turn: it can `SELECT` over the context, define prompt
functions, call tools with `CALL`, delegate with `rlm(...)` and
`spawn(...)`, and finish with `FINAL`. Every model call is memoised, so
running the same task again is free.

Watch it live, or explore the plan space without spending anything:

```bash
kleene tui --run @demos/oolong/task.txt --context demos/oolong/corpus.txt
kleene explain "SELECT c FROM candidates WHERE llm_bool('Is ' || c || ' a real place?')"
kleene repl < demos/planner/three_way.sql     # join ordering over call predicates
```

Keep it learning overnight, and measure it:

```bash
kleene learn run --tasks 20 --generators puzzle,corpus   # resumable; state is tables in the store
kleene learn report                                       # the morning query
kleene bench run tasks/terminal --mode frozen             # a task pack, one mode
kleene bench report                                       # accuracy and cost per pack and mode
```

Everything lands in `.kleene/run.duckdb` under the current directory.

## Commands

| Command | Does |
|---|---|
| `kleene run <task>` | drive a model through the SQL turn loop to `FINAL` |
| `kleene resume <run-id>` | continue a run that hit its turn cap |
| `kleene repl [-c SQL]` | CallSQL against the store, interactive or scripted |
| `kleene explain <sql>` | the call plan and its cost, without executing |
| `kleene trace <sql>` | DuckDB SQL over the trace, memo and session tables |
| `kleene tui` | the terminal UI over the engine daemon: call tree, transcript, plan, trace explorer, task board |
| `kleene attach` | the same, headless: every event as a JSON line |
| `kleene daemon` | the engine as a server on a Unix socket |
| `kleene learn …` | the continual loop: tasks, oracles, ratings, curriculum, playbook |
| `kleene bench …` | task packs under learning, frozen and plain-agent modes |

Details and every flag: [`docs/CLI.md`](docs/CLI.md).

## How it works

```
model reply ─▶ kleene-sql ─▶ kleene-algebra ─▶ budget check ─▶ kleene-exec ─▶ rendered rows
   CallSQL     parse, resolve     call kinds, cost,   refuse if over    operators, recursion,   back to the model
               to a LogicalPlan   rules, EXPLAIN                        calls via LiveSink
                                                                            │
                                    kleene-llm  (Anthropic, OpenAI-compatible, router, replay)
                                    kleene-tools (files, grep, read, shell, write_file, patch, git_*, web_*)
                                    child sessions  (rlm, spawn: same loop at depth + 1 with a role and a budget slice)
                                                                            │
                                    kleene-store (DuckDB): tables · memo · trace_* · sessions · learning · evals
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
git clone https://github.com/MarcusElwin/callgebra && cd kleene
cargo build                                                          # DuckDB compiles once, ~10 min
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

`CLAUDE.md` holds the working rules (interfaces first, typed errors, the
flag shapes that keep DuckDB from rebuilding) and `ci/README.md` the release
workflow. Written in Rust, model-agnostic with no SDK and no gateway
required, MIT licensed.

## Why "Kleene"

Stephen Cole Kleene gave computation two of its load-bearing ideas. The
**Kleene star** turns "one step" into "any number of steps": `a*` is the
closure of `a` under repetition, and it is exactly what a recursive CTE
computes when it runs a term to its fixpoint. The **Kleene fixed-point
theorem** says how to reach that closure: start from nothing and apply the
step until nothing changes, which is the semi-naive evaluation the executor
runs. His **recursion theorem** shows a program can refer to itself without
paradox, which is what a session does when it opens a child session with
`rlm(...)`.

That is this project in three theorems. A model call is a step; SQL gives
it joins, predicates and aggregation; recursion with a beam gives it search;
the planner prices the closure before it is computed. The engine is named
for the mathematician who showed that closure is a thing you can compute,
and the dialect keeps its own name, CallSQL, because the SQL is where the
calls are.
