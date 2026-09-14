# Architecture

How Kleene is put together: the crates, the path a statement takes from
the model's reply to rows in DuckDB, and the processes that run it. The design
rationale is in [`PLAN.md`](PLAN.md); the language is in
[`DIALECT.md`](DIALECT.md); this document is the map of the code as built.

## One paragraph

A **session** is a loop: the harness sends the model a cached system prefix
(the CallSQL rules, the catalog of tables, functions, tools and agents, the
budget) plus the transcript; the model replies with CallSQL inside one
```` ```sql ```` fence; the harness parses it, annotates every operator with
the model and tool calls it implies, prices that plan, refuses it if the
remaining budget cannot pay, executes it, renders the rows back into the
transcript, and repeats until the model writes `FINAL`. Child sessions for
`rlm(...)` and `spawn(...)` are the same loop one level deeper with a role,
a budget slice and their own table namespace. Every table, memo entry and
trace event lives in one DuckDB file, so the trace is queryable with the same
SQL, and the learning loop's state (tasks, ratings, playbook) is tables too.

## Crates

```
kleene            CLI binary: run, resume, repl, explain, trace, tui, attach, daemon, learn, bench
├── kleene-tui    ratatui client over the daemon protocol
├── kleene-daemon Unix-socket server, JSONL protocol with cursors, client
└── kleene-harness
    │                sessions and the turn loop, LiveSink (memo → provider → budget → trace),
    │                Repl, prompt rendering, agent roles, the continual loop (learn/), bench
    ├── kleene-exec      operators, three-valued logic, semi-naive recursion, concurrent calls
    │   └── kleene-algebra   LogicalPlan → CallPlan: call kinds, cost model, rules, EXPLAIN
    │       └── kleene-sql   sqlparser → validated subset → name/type resolution → LogicalPlan
    ├── kleene-llm       Provider trait, Anthropic and OpenAI-compatible adapters, router, replay
    ├── kleene-tools     table functions and CALL tools with volatility labels, workspace jail
    ├── kleene-store     DuckDB: session tables, memo, trace tables, catalog metadata
    └── kleene-trace     TraceEvent, Tracer, sinks (store, fan-out)
kleene-core       shared interface types: Value, Batch, Schema, Catalog, Budget, CallKind, ids
kleene-difftest   proptest generator + DuckDB oracle for the relational core
```

Every crate depends on `kleene-core` and nothing else depends on the
binary. `kleene-core` changes deliberately and first (see `CLAUDE.md`).

| Crate | Owns | Key types |
|---|---|---|
| `kleene-core` | The vocabulary every crate shares | `Value`, `DataType`, `Schema`, `Batch`, `Catalog`, `FunctionDef`, `CallKind`, `Volatility`, `Budget`, `BudgetUsage`, `RunId`/`SessionId`/`StatementId` |
| `kleene-sql` | Parsing and planning to a positional logical plan; every rejection is an `SqlError` with a hint | `plan_sql()`, `LogicalPlan`, `Statement`, `SqlError`, `render_error()` |
| `kleene-algebra` | Call kinds on operators, estimates, rewrite rules, plan search, `EXPLAIN` text | `annotate()`, `CallPlan`, `Rule`, `Rewrite`, `explain()` |
| `kleene-exec` | Executing a plan against a `CallSink` | `execute()`, `execute_statement()`, `CallSink`, `MemorySink`, `ExecError` |
| `kleene-llm` | Talking to models over the wire, no SDKs | `Provider`, `CompletionRequest`, `AnthropicProvider`, `OpenAiCompatProvider`, `RoutedProvider`, `RouterConfig`, `Pricing`, `ReplayProvider`, `RecordingProvider`, `provider_from_env()` |
| `kleene-tools` | Tools as table functions and statements | `ToolRegistry`, `standard_tools()`, `ToolContext`, `catalog_entries()` |
| `kleene-store` | DuckDB behind a mutex on the blocking pool | `DuckDbStore`, `DuckDbTraceSink`, `MemoryStore` |
| `kleene-trace` | The event model | `TraceEvent`, `Tracer`, `TraceSink`, `FanoutSink` |
| `kleene-harness` | Sessions, the REPL, prompts, roles, learning, benchmarks | `Harness`, `HarnessConfig`, `Session`, `Repl`, `LiveSink`, `StoreSink`, `AgentRole`, `learn::Learn` |
| `kleene-daemon` | The engine as a server | `Daemon`, `Client`, `ClientRequest`, `ServerMessage`, `Cursor`, `EventLog` |
| `kleene-tui` | The terminal client and the setup wizard | `App`, `Model`, `View`, `Theme`, `setup::SetupApp`, `headless()` |
| `kleene` | The CLI | `main.rs` only |
| `kleene-difftest` | Property-based equivalence with DuckDB | `generator`, `run`, `compare` |

## The path of one statement

```
model reply ──extract_sql──▶ CallSQL text
                                 │
                                 ▼
  kleene-sql   sqlparser 0.62 AST ─▶ subset validation ─▶ CallSQL extensions
                  (CREATE FUNCTION … AS PROMPT, CREATE AGENT, CALL, SET, FINAL, EXPLAIN)
                  ─▶ name and type resolution against the Catalog ─▶ LogicalPlan
                                 │
                                 ▼
  kleene-algebra  annotate(LogicalPlan, Catalog, Stats) ─▶ CallPlan
                     every operator carries: rows, calls, tokens, dollars, depth,
                     call kinds (λ scalar, κ table, ρ recursive, tool), fences
                     rules run to a fixpoint: cheap-first, semi-join, cascade,
                     beam recursion, join ordering (once, before the loop), fences
                     explain() renders the plan, the alternatives and the plan space
                                 │
                                 ▼
  harness Repl       budget check: estimate > remaining ⇒ refuse with the plan
                                 │
                                 ▼
  kleene-exec     execute(plan, ExecContext) ─▶ stream of Batches
                     materialising operators, semi-naive recursion with beams,
                     rows evaluated concurrently up to call_concurrency
                        │ scalar_call / table_call / tool_call / child session
                        ▼
  harness LiveSink   memo lookup ─▶ router picks a candidate ─▶ adapter over reqwest
                     ─▶ usage priced ─▶ budget charged ─▶ TraceEvent emitted
                     ─▶ memo stored   (rlm/spawn: ChildRunner opens a child session)
                                 │
                                 ▼
  harness render     rows (truncated to 20 × 200 chars by default) + a footer
                     (calls, tokens, dollars, remaining budget) back into the transcript
```

`kleene repl` and `kleene explain` run the same path without a model
turn around it. `EXPLAIN` stops after annotation; `EXPLAIN ANALYZE` runs and
prints actuals beside estimates.

### The catalog

`Catalog` is the session's view of the world: tables with declared CallSQL
types, builtin and user functions with their `CallKind` and `Volatility`,
tools, and declared agents. The planner resolves names against it, the
annotator prices calls from it, the prompt renderer groups it by call kind
and volatility for the model. A child session sees a **restricted** catalog
(the role's tools only), which is the security boundary for delegation.

### Call kinds and volatility

Each function or tool is `IMMUTABLE`, `STABLE` or `VOLATILE`, and the planner
trusts the label: rules move predicates freely across immutable and stable
operators and never across a volatile one (a fence). Prompt functions default
to immutable (same prompt, same answer, so the memo applies), SQL bodies to
stable, shell bodies and every side-effecting tool to volatile. Volatile
tools are only allowed under `CALL`, never in a `FROM`.

## Sessions and delegation

```
Harness::run(task, context)
  └─ root Session (depth 0, role "root", full catalog, whole budget)
       turn 1: system prefix + task ─▶ model ─▶ SQL ─▶ Repl ─▶ rendered rows
       turn 2: … + transcript ─▶ …
       ├─ CROSS JOIN LATERAL rlm(q, ctx)   ─▶ one child per input row, depth 1, worker tier
       ├─ CROSS JOIN LATERAL spawn('reviewer', task) ─▶ child with the role's tools and budget
       └─ FINAL FROM (query)               ─▶ the answer relation
```

- **Prefix caching.** The system prefix is deterministic for the same inputs
  and marked cacheable for providers that support it, so a long transcript
  costs new tokens only for what changed.
- **Namespaces.** A child's tables are `cgs_<id>__name` in the shared store;
  `ctx` is preloaded one row per paragraph. Its `FINAL` comes back to the
  parent as `(answer, detail JSON, session)`; a child that never reached
  `FINAL` returns one row with a NULL answer and the outcome in `detail`, so
  the parent's statement survives.
- **Budgets.** `Budget` has calls, tokens, dollars, depth and wall clock. A
  child gets the parent's remaining slice intersected with its role's budget;
  its spending rolls up. Depth is enforced at the call site.
- **Persistence and resume.** `kleene_sessions` is written after every
  turn (transcript, functions, agents, settings, usage). `kleene resume`
  continues a root session that hit its turn cap or was interrupted.
- **Cancellation.** `LiveSink::cancel_statement` is checked before every model
  and tool call; the model sees a cancelled statement as an error and goes on.

## Store and trace

One DuckDB file (`.kleene/run.duckdb` by default) holds everything:

| Tables | Written by | Read by |
|---|---|---|
| user tables, `ctx`, `cgs_*__*` | statements | statements |
| `kleene_columns` | store | planner (declared CallSQL types; JSON is stored as VARCHAR) |
| `memo` | `LiveSink` | `LiveSink` (keyed by model and prompt fingerprint, across runs) |
| `trace_runs`, `trace_sessions`, `trace_statements`, `trace_calls`, `trace_tool_calls`, `trace_rounds`, `trace_final` | `DuckDbTraceSink` from a background task | `kleene trace`, the TUI explorer, `EXPLAIN`'s sampled selectivity |
| `kleene_sessions` | harness after every turn | `kleene resume` |
| `tasks`, `task_ratings`, `solver_ratings`, `generator_state`, `playbook`, `playbook_evals`, `attempts`, view `trace_tasks` | `learn` | `learn board/report/playbook`, TUI view 4 |
| `evals`, `bench_runs` | `bench` | `bench report/curve/csv` |

`TraceEvent`s flow through a `Tracer` to a sink. In-process that is the store
sink; under the daemon a `FanoutSink` mirrors them into the event log as
well, so the TUI and the trace tables tell the same story. The store flushes
the async writer before answering a `Query`, so the explorer never lags.

## Processes

```
kleene run / repl / explain / learn / bench      one process, in-process harness

kleene daemon ──── .kleene/daemon.sock ────┬─ kleene tui      (ratatui client)
   Daemon: EventLog with Cursor{generation,seq}   ├─ kleene attach   (JSONL, headless)
   Harness + store + provider                     └─ any JSONL client
```

The protocol (version 2) is newline-delimited JSON over a Unix socket.
Requests: `Subscribe { after }`, `StartRun`, `Submit` (REPL statements for a
client-owned session), `Query` (DuckDB SQL over the store, with a `tag`),
`Cancel`, `CancelRun`, `ListRuns`, `Detach`. Replies: `Hello`, `Event` (a
trace event with its cursor), `CallDelta`, `RunAccepted`, `RunFinished`,
`Submitted`, `Table`, `Runs`, `Ok`, `Error`. A client that reconnects sends
its last cursor and gets replay from there; a stale generation replays
everything. `kleene tui` starts a daemon in the background if none is
listening.

## Model layer

`Provider` is one trait (`complete`, streaming optional). Two adapters speak
the wire formats directly over `reqwest`: the Anthropic Messages API and the
OpenAI-compatible chat API (OpenAI, local servers, gateways). `RoutedProvider`
resolves an **alias** (`root`, `worker`, `proxy`, `judge`, or your own) to an
ordered list of `(provider, model)` candidates with default options, skips
candidates whose circuit breaker is open, prices usage from a `Pricing`
table, and is what the harness holds. `ReplayProvider` serves recorded
fixtures for tests and offline benchmarks; `RecordingProvider` writes them.

Configuration is `ProviderSettings`: the config file `kleene setup` writes
(`~/.config/kleene/config.toml`, owner-readable) with the environment layered
on top field by field, so `KEY=... kleene run` still wins. The setup wizard
itself lives in `kleene-tui::setup` as a pure state machine with a renderer,
shared by `kleene setup` and the TUI's first start; see
[`CLI.md`](CLI.md#configuring-a-model-provider).

## Planner

`kleene-algebra` runs rules over the annotated plan until nothing
changes. Join ordering runs once first: it collects the relations under a
tree of inner and cross joins, then does dynamic programming over subsets with
branch-and-bound, pricing each split with the predicates that first become
applicable there (pure conjuncts narrow the pairs, call conjuncts pay one call
per surviving pair). Cheap-first orders conjuncts to mirror the executor's
left-to-right short-circuit. Cascade rewrites an oracle predicate with a
declared proxy into a band. Semi-join turns `[NOT] EXISTS` with a call into
a semi or anti join that stops at the first (counter)example. Beam recursion
simulates the fixpoint round by round so a trailing `ORDER BY … LIMIT k` caps
every frontier. Estimates use the store's row counts, sampled selectivities
(observed pass rates of boolean call predicates in this session), per-alias
cost factors and pricing.

## The continual loop and benchmarks

`harness::learn` keeps the loop's state in the store. Generators
(`sat3`, `graph`, `puzzle`, `corpus`, `repo`, `statements`, `contracts`)
produce tasks deterministically from a dial and a seed, each with a code
oracle (`verify`). The curriculum picks the pending task nearest even odds
for the current solver rating (Bradley-Terry over tasks and solver
configurations), runs it through `Harness`, judges `FINAL`, moves both
ratings and the generator's dial. SQL that solved a task becomes a playbook
candidate and is adopted only after a replay eval with memo bypassed wins on
solves and cost. `learn::bench` runs task packs (`tasks/*/pack.json`) under
`learning`, `frozen` and `plain` modes; `plain` is a tool-calling agent on
the same provider, tools and budget, the baseline for cost parity.

## Testing strategy

- The relational core is tested **differentially against DuckDB**
  (`kleene-difftest`): proptest generates schemas, data and queries in the
  CallSQL/DuckDB intersection and compares multisets, plus a curated corpus
  of shrunk regressions.
- Model-facing text (`EXPLAIN`, rendered tables, error messages) is part of
  the interface and asserted verbatim.
- Anything that needs a model uses scripted or replay providers; no test
  talks to a network.
- Crates that link DuckDB keep one integration-test binary each
  (`tests/all/main.rs`) because every such binary carries the engine.

## Where to look

| Question | File |
|---|---|
| What SQL is accepted | `crates/kleene-sql/src/planner.rs`, `extensions.rs` |
| How a plan is priced and rewritten | `crates/kleene-algebra/src/annotate.rs`, `rules.rs` |
| What the model reads each turn | `crates/kleene-harness/src/prompt.rs` |
| The turn loop and children | `crates/kleene-harness/src/session.rs` |
| Memo, budget, trace per call | `crates/kleene-harness/src/live.rs` |
| Wire formats | `crates/kleene-llm/src/adapters/` |
| Tool semantics and the workspace jail | `crates/kleene-tools/src/tools/`, `paths.rs` |
| Store schema | `crates/kleene-store/src/duckdb.rs` |
| Daemon protocol | `crates/kleene-daemon/src/lib.rs` |
| TUI layout, keys, theme | `crates/kleene-tui/src/ui.rs`, `lib.rs`, `theme.rs` |
| Provider config file and the setup wizard | `crates/kleene-llm/src/config.rs`, `crates/kleene-tui/src/setup.rs` |
| The learning loop | `crates/kleene-harness/src/learn/` |
| CLI wiring | `crates/kleene/src/main.rs` |
