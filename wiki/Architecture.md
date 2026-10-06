# Architecture

The map of the code as built. The design rationale is [`docs/PLAN.md`](https://github.com/MarcusElwin/kleene/blob/main/docs/PLAN.md); the full version of this page is [`docs/ARCHITECTURE.md`](https://github.com/MarcusElwin/kleene/blob/main/docs/ARCHITECTURE.md).

## One paragraph

A **session** is a loop. The harness sends the model a cached system prefix (the CallSQL rules, the catalog of tables, functions, tools and agents, the budget) plus the transcript; the model replies with CallSQL inside one fence; the harness parses it, annotates every operator with the model and tool calls it implies, prices that plan, refuses it if the remaining budget cannot pay, executes it, renders the rows back into the transcript, and repeats until the model writes `FINAL`. Child sessions for `rlm(...)` and `spawn(...)` are the same loop one level deeper with a role, a budget slice and their own table namespace. Every table, memo entry and trace event lives in one DuckDB file, so the trace is queryable with the same SQL.

## Crates

```
kleene            CLI binary: run, resume, repl, explain, trace, tui, attach, daemon, learn, bench
├── kleene-tui    ratatui client over the daemon protocol, the setup wizard
├── kleene-daemon Unix-socket server, JSONL protocol with cursors
└── kleene-harness
    │                sessions and the turn loop, LiveSink (memo → provider → budget → trace),
    │                Repl, prompt rendering, agent roles, the continual loop (learn/), bench
    ├── kleene-exec      operators, three-valued logic, semi-naive recursion, concurrent calls
    │   └── kleene-algebra   LogicalPlan → CallPlan: call kinds, cost model, rules, EXPLAIN
    │       └── kleene-sql   sqlparser → validated subset → name/type resolution → LogicalPlan
    ├── kleene-llm       Provider trait, Anthropic and OpenAI-compatible adapters, router, replay
    ├── kleene-tools     table functions and CALL tools with volatility labels, workspace jail
    ├── kleene-store     DuckDB: session tables, memo, trace tables, catalog metadata
    └── kleene-trace     TraceEvent, Tracer, sinks
kleene-core       shared interface types: Value, Batch, Schema, Catalog, Budget, CallKind, ids
kleene-difftest   proptest generator + DuckDB oracle for the relational core
```

Every crate depends on `kleene-core` and nothing else depends on the binary.

## The path of one statement

1. **Extract**: the SQL fence is lifted out of the model's reply.
2. **`kleene-sql`**: sqlparser AST, subset validation, the CallSQL extensions (`CREATE FUNCTION ... AS PROMPT`, `CREATE AGENT`, `CALL`, `SET`, `FINAL`, `EXPLAIN`), name and type resolution against the catalog, a `LogicalPlan`.
3. **`kleene-algebra`**: `annotate()` gives every operator rows, calls, tokens, dollars, depth, call kinds (λ scalar, κ table, ρ recursive, tool) and fences; rules run to a fixpoint (cheap-first, semi-join, cascade, beam recursion, join ordering, fences); `explain()` renders it.
4. **Budget check** in the harness: estimate above the remaining budget means refusal with the plan.
5. **`kleene-exec`**: executes the plan as a stream of batches, with semi-naive recursion and beams, rows evaluated concurrently up to the call concurrency.
6. **`LiveSink`** per call: memo lookup, router picks a candidate, adapter over reqwest, usage priced, budget charged, trace event emitted, memo stored. `rlm` and `spawn` open a child session here.
7. **Render**: rows (truncated to 20 × 200 characters by default) plus a footer with calls, tokens, dollars and remaining budget go back into the transcript.

`kleene repl` and `kleene explain` run the same path without a model turn around it.

## Call kinds and volatility

Each function or tool is `IMMUTABLE`, `STABLE` or `VOLATILE`, and the planner trusts the label: rules move predicates freely across immutable and stable operators and never across a volatile one. Prompt functions default to immutable (same prompt, same answer, so the memo applies), SQL bodies to stable, shell bodies and side-effecting tools to volatile. Volatile tools are only allowed under `CALL`, never in a `FROM`.

## Sessions and delegation

- **Prefix caching**: the system prefix is deterministic for the same inputs and marked cacheable, so a long transcript costs new tokens only for what changed.
- **Namespaces**: a child's tables are `cgs_<id>__name` in the shared store. Its `FINAL` comes back as `(answer, detail JSON)` for `rlm` and `(answer, detail JSON, session)` for `spawn`; a child that never reached `FINAL` returns a NULL answer with the outcome in `detail`, so the parent's statement survives.
- **Budgets**: calls, tokens, dollars, depth and wall clock. A child gets the parent's remaining slice intersected with its role's budget; spending rolls up.
- **Persistence**: `kleene_sessions` is written after every turn, which is what `kleene resume` reads.
- **Cancellation** is checked before every model and tool call; the model sees a cancelled statement as an error and goes on.

## Store and trace

One DuckDB file holds everything:

| Tables | Written by | Read by |
|---|---|---|
| user tables, `ctx`, `cgs_*__*` | statements | statements |
| `memo` | `LiveSink` | `LiveSink`, keyed by model and prompt fingerprint, across runs |
| `trace_runs`, `trace_sessions`, `trace_statements`, `trace_calls`, `trace_tool_calls`, `trace_rounds`, `trace_final` | the trace sink | `kleene trace`, `/trace`, the planner's sampled selectivity |
| `kleene_sessions` | harness after every turn | `kleene resume` |
| `tasks`, `task_ratings`, `solver_ratings`, `generator_state`, `playbook`, `playbook_evals`, `attempts` | `learn` | `learn board/report/playbook`, `/board` |
| `evals`, `bench_runs` | `bench` | `bench report/results/plot` |

## Processes

`kleene run`, `repl`, `explain`, `learn` and `bench` run the harness in one process. The TUI talks to an engine daemon over a Unix socket and a newline-delimited JSON protocol with cursors: a client that reconnects sends its last cursor and gets replay from there, and `kleene attach` watches the same events headless. Requests are `Subscribe`, `StartRun`, `Submit`, `Query`, `Cancel`, `CancelRun`, `ListRuns`, `Reload` and `Detach`; replies include `Event`, `CallDelta`, `TurnFinished`, `RunFinished` and `Table`.

## Model layer

`Provider` is one trait. Two adapters speak the wire formats directly over reqwest: the Anthropic Messages API and the OpenAI-compatible chat API (OpenAI, local servers, gateways); a third, behind the `gateway` feature, speaks the Open Responses API. `RoutedProvider` resolves an alias to an ordered list of candidates, skips those whose circuit breaker is open and prices usage. `ReplayProvider` serves recorded fixtures for tests and offline benchmarks; no test talks to a network.

## Testing

The relational core is tested differentially against DuckDB: proptest generates schemas, data and queries in the CallSQL/DuckDB intersection and compares multisets. Model-facing text (`EXPLAIN`, rendered tables, error messages) is part of the interface and asserted verbatim.

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
| The learning loop | `crates/kleene-harness/src/learn/` |
