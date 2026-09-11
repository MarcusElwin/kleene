# Callgebra — implementation plan

> **Callgebra**: relational algebra for recursive model calls.
> The model writes SQL. SQL compiles to a *call algebra*: relational operators
> annotated with the language-model and tool calls they imply. A planner rewrites
> and costs that algebra, an async executor runs it as a graph of LLM calls,
> recursive sub-sessions and tool calls, and the execution trace is itself a
> relation you can query. A TUI shows the whole thing live.

Status: plan only. Nothing is implemented yet. See `docs/RESEARCH.md` for the
sources behind the choices made here.

---

## 1. Thesis in one page

**Why SQL.** Recursive Language Models (RLMs) give the model a REPL whose
variables hold the context, and let it write code that calls itself on slices of
that context. The official RLM harness and every reimplementation use Python.
SQL is a better language for *this specific job* for three reasons:

1. **It is declarative.** The model says *what* relation it wants; the harness
   decides *how many calls* to make, in what order, in parallel or not, with
   which model. That is exactly the freedom a planner needs and Python denies.
2. **It has a real algebra with known complexity classes.** Select-project-join
   (conjunctive queries) is NP-complete in combined complexity; full relational
   algebra is PSPACE-complete; `WITH RECURSIVE` reaches Datalog-and-beyond, and
   with `UNION ALL` and unbounded values SQL is Turing complete (Gierth's cyclic
   tag system in SQL:2003). We can label every query the model writes with the
   fragment it lives in, and we can deliberately construct queries whose
   *planning* is a combinatorial search (join ordering over LLM predicates is
   NP-hard, as for ordinary joins).
3. **The trace is a relation.** Calls, spans, tokens, costs and depths land in
   tables. The model, the harness and the human all inspect execution with the
   same language they use to drive it.

**What the model actually sees.** A schema catalog: virtual tables for the
filesystem, search, its own context, memory and the trace; scalar functions
that cost one model call each (`LLM`, `LLM_BOOL`, `LLM_JSON`, and prompt-defined
functions such as `VERIFY`/`REFUTE`); table functions that fan out
(`RLM(question, context)` for a recursive sub-session, `EXPAND(prompt, n)` for
successor generation, `grep`, `files`, `chunks`, `web_search`, `shell`). It
answers a task by iterating SQL statements in a REPL, materialising
intermediate tables, and finishing with `FINAL(...)`.

**Where it becomes research.** Every construct maps to a call pattern with a
cost signature. `WHERE VERIFY(c)` is a map over candidates. `AND NOT EXISTS
(SELECT 1 FROM counterexamples WHERE REFUTE(c, ce))` is a counterexample-guided
semi-join, the CEGIS loop expressed relationally. A recursive CTE whose
recursive term calls `EXPAND` is breadth-first search with an LLM successor
function, and its depth, branching factor and frontier size are measurable
plan properties. The planner must choose between cheap-model cascades, batching,
memoisation, semi-joins and materialisation, and the cost of choosing grows
combinatorially with the query. The harness measures and visualises all of it,
and a continual loop feeds it procedurally generated and self-proposed hard
instances whose difficulty is dialled to keep the system at its frontier.

---

## 2. Architecture

```
                      ┌──────────────────────────────────────────┐
  task / user ──────▶ │  harness  (RLM REPL loop, continual loop)│
                      └───────┬──────────────────────────▲───────┘
                              │ SQL text                 │ truncated result,
                              ▼                          │ errors, budget
        ┌──────────────── callgebra-sql ───────────────┐ │
        │ sqlparser 0.62 → subset validation →         │ │
        │ name resolution against Catalog → LogicalPlan│ │
        └───────────────────────┬──────────────────────┘ │
                                ▼                        │
        ┌──────────────── callgebra-algebra ───────────┐ │
        │ LogicalPlan → CallPlan (operators × CallKind) │ │
        │ cost model · rewrite rules · plan search      │ │
        │ EXPLAIN (estimated calls / tokens / $ / depth)│ │
        └───────────────────────┬──────────────────────┘ │
                                ▼                        │
        ┌──────────────── callgebra-exec ──────────────┐ │
        │ async pull operators (tokio) · semi-naive    │ │
        │ recursion · batching · memo · budgets ·      │─┘
        │ cancellation · concurrency limits            │
        └──────┬───────────────┬───────────────┬───────┘
               ▼               ▼               ▼
       callgebra-llm    callgebra-tools   SPAWN / RLM sub-agent (depth+1)
       Provider trait:  (fs, grep, shell, ─▶ back into harness
        Aura gateway     web, sandbox)      with a role, budget slice
        (Open Responses)                    and catalog subset
        Anthropic direct
        OpenAI-compatible
        mock / replay
               │               │               │
               └───────────────┴───────────────┘
                               ▼
        ┌──────────────── callgebra-trace ─────────────┐
        │ append-only events → SQLite (rusqlite)       │
        │ exposed back to queries as trace_* relations │
        └───────────────────────┬──────────────────────┘
                                ▼
        ┌──────────────── callgebra-tui ───────────────┐
        │ ratatui 0.30 · call tree · transcript ·      │
        │ plan view · metrics · trace explorer · tasks │
        └──────────────────────────────────────────────┘
```

The engine runs as a **daemon** (Unix socket, newline-delimited JSON events with
monotonic cursors) and the TUI is a thin client that can detach and reattach.
This is borrowed from Prime Agent's supervisor/worker split and tau's
server-plus-TUI design, and it also gives us a `--headless` JSONL mode for
scripting and evaluation.

---

## 3. Key decisions

### 3.1 Own engine over sqlparser-rs, not DataFusion

| Option | Verdict | Why |
|---|---|---|
| **Hand-written interpreter over the `sqlparser` AST** | **Chosen** | Full control of the call algebra, of where async calls sit in the tree, of recursion and budgets. Small binary, fast builds. Every operator is ours to instrument. |
| Apache DataFusion 55 | Rejected for v1 | Async scalar UDFs are hoisted only under Projection and Filter, table-function arguments are plan-time literals (no per-row fan-out), the memo-based optimizer is not what we want to study, and it pulls ~400 crates and minutes of compile time. Its `SqlToRel` planner (name resolution, coercion, decorrelation) remains an option for a later frontend swap. |
| SQLite with UDFs and virtual tables | Used as **oracle and store**, not as engine | UDFs are synchronous, so calls would serialise, and there is no plan-level control. But it gives us persistence of session tables and the trace, and a reference semantics: pure-relational queries are differential-tested against SQLite. |
| GlueSQL / Polars SQL | Rejected | Sync core (GlueSQL 0.20), no recursive CTEs, no LATERAL, Polars SQL is explicitly internal. |

### 3.2 SQL dialect: "CallSQL"

A strict subset of PostgreSQL-flavoured SQL, parsed with `PostgreSqlDialect`,
then validated. Anything outside the subset is a clear error with a hint, which
is returned to the model as a result row rather than crashing the turn.

Supported in v1:

- `SELECT` with projections, expressions, aliases, `DISTINCT`, `ORDER BY`, `LIMIT`/`OFFSET`
- `FROM` tables, subqueries, table functions, `LATERAL`, `CROSS/INNER/LEFT JOIN ... ON`
- `WHERE` with `AND/OR/NOT`, comparisons, `IN (subquery)`, `EXISTS`/`NOT EXISTS`, correlated subqueries
- `GROUP BY` with `COUNT/SUM/MIN/MAX/AVG/STRING_AGG`, `HAVING`
- `WITH` and `WITH RECURSIVE` (`UNION` and `UNION ALL`), `MAXRECURSION n`
- `CREATE TABLE t AS SELECT ...`, `INSERT INTO t SELECT ...`, `DROP TABLE`
- `CREATE FUNCTION f(args) RETURNS type AS PROMPT '...'` (prompt-defined UDF; also `AS SQL (...)` and `AS SHELL '...'`)
- `EXPLAIN [ANALYZE] SELECT ...`
- `SET budget.calls = 200`, `SET model.default = 'claude-opus-5'`, `SET effort = 'low'`
- `FINAL(expr)` / `FINAL FROM (SELECT ...)` to end a session
- `CREATE AGENT name MODEL '...' EFFORT '...' TOOLS (...) BUDGET (calls n, tokens m) PROMPT '...'`
- `SEND(agent_handle, text)` / `INBOX` for messages between parent and children (see 3.7)

Built-in call functions (each is a `CallKind`, see 3.3):

| Function | Kind | Notes |
|---|---|---|
| `LLM(prompt [, model, effort]) -> TEXT` | scalar call | one Messages request per distinct argument tuple |
| `LLM_BOOL(prompt) -> BOOL` | scalar call | structured output, used by `VERIFY`/`REFUTE` |
| `LLM_JSON(prompt, schema) -> JSON` | scalar call | `output_config.format` JSON schema |
| `EMBED(text) -> VECTOR`, `SIM(a, b)` | scalar call / pure | for semantic joins with proxy filtering |
| `RLM(question, context) -> TABLE(answer, ...)` | recursive call | spawns a child session at depth+1 with `context` preloaded as table `ctx`; sugar for `SPAWN(self, ...)` |
| `SPAWN(agent, task, context) -> TABLE(answer, handle, ...)` | recursive call | runs a declared agent role as a child session; one child per input row, concurrently |
| `EXPAND(prompt, n) -> TABLE(item)` | table call | LLM-generated successors, the branching step in searches |
| `files(glob)`, `lines(path)`, `grep(path, pattern)`, `chunks(text, size)` | tool | filesystem, pure |
| `shell(cmd)`, `web_search(q)`, `web_fetch(url)` | tool | sandboxed / rate-limited |
| `trace_calls`, `trace_spans`, `trace_stmts` | virtual tables | the run's own trace, live |

`VERIFY` and `REFUTE` are not built-ins. They are prompt-defined functions the
task (or the model) declares, so the harness can swap in a code oracle
(`AS SHELL`) when one exists and an LLM judge when one does not.

### 3.3 The call algebra

Ordinary relational operators: σ (filter), π (project), ⋈ (join), ∪, −, γ
(aggregate), μ (least fixpoint, for recursive CTEs). Callgebra adds a
*call kind* to any operator that evaluates a call expression:

| Operator | Meaning | Cost signature |
|---|---|---|
| λ_f (map-call) | scalar call `f` per distinct input tuple | `|distinct(R)| × tokens(f)` |
| κ_g (expand-call) | table call `g` per input tuple, i.e. `LATERAL g(...)` | `|R| × tokens(g)`, output cardinality = branching factor |
| ρ_d (recurse) | `RLM(...)` at depth d | recursive: the child's whole plan |
| σ_llm, ⋈_llm | filter/join whose predicate contains a call | inherits λ; join is `|R × S|` calls unless rewritten |

Rewrite rules the planner knows (all are standard query-rewrite shapes applied
to call costs):

1. **Cheap first**: `σ_pure ∘ σ_llm ≡ σ_llm ∘ σ_pure`, always push the pure predicate below the call.
2. **Dedupe then map**: `λ_f(R) ≡ λ_f(δ(R)) ⋈ R`, so duplicate prompts cost one call. Backed by a persistent memo keyed on `(model, prompt, schema)`.
3. **Batch**: `λ_f` over n tuples becomes one call with n items when the prompt template is marked batchable; the planner picks batch size from the token budget.
4. **Cascade**: `σ_oracle(R) ≈ σ_oracle(σ_proxy≥τ_hi(R)) ∪ σ_proxy≥τ_lo(R)` with a cheap model or embedding as proxy; thresholds set from a sample, as in LOTUS.
5. **Semi-join for EXISTS**: `NOT EXISTS` with a call predicate becomes an anti-semi-join that stops on the first refuting counterexample (short-circuit), rather than materialising the cross product.
6. **Frontier limiting**: inside μ, a `LIMIT` or `ORDER BY score LIMIT k` in the recursive term becomes beam search; `MAXRECURSION` bounds depth.
7. **Join ordering with call predicates**: dynamic programming over join orders where each candidate order has a call cost. This is where the plan space explodes and where the project deliberately looks.

`EXPLAIN` prints the CallPlan as a tree with per-node estimated calls, tokens,
dollars and depth, and the fragment class of the query (CQ / FO / recursive).
`EXPLAIN ANALYZE` prints actuals beside estimates. The harness runs `EXPLAIN`
on every statement before execution and refuses statements that exceed the
remaining budget, returning the estimate to the model instead.

### 3.4 Execution model

- **Pull-based async operators** (`Stream<Item = Result<Batch>>`), `Batch` is a
  small columnar chunk of `Value` (`Null | Bool | Int | Float | Text | Json | Vector`).
- **Recursion** by semi-naive evaluation: the delta of each round is joined
  against the recursive term; LLM calls in the recursive term run only on the
  delta. A round is one awaitable unit, so the TUI shows frontier size per round.
- **Concurrency**: a per-session semaphore for LLM calls and one for tools; a
  global rate limiter per provider. Ordered results are preserved with
  `buffered` streams.
- **Budgets**: calls, tokens, dollars, depth, wall clock, per session and per
  statement, inherited and subdivided by child sessions (a child gets a slice of
  its parent's remaining budget). Exceeding a budget cancels the statement, not
  the session, and returns a budget row to the model.
- **Memo**: SQLite table `memo(model, prompt_hash, schema_hash, output, usage)`,
  shared across sessions and runs. Replaying a run against a warm memo is free,
  which is how the test-suite runs real prompts without real calls.
- **Session state**: every `CREATE TABLE` lands in the session's SQLite file.
  Kill the process, restart, `callgebra resume <session>`, the tables and the
  transcript are still there. Checkpointing is a property of the storage, not a
  feature bolted on.

### 3.5 The RLM loop on SQL

```
system prompt (cached prefix):
  catalog (tables, functions, their costs), CallSQL rules, examples, budget
user turn:
  task text, and note that `ctx` holds the context (n rows, m tokens)
assistant turn:
  ```sql  ...one or more statements...  ```
harness:
  parse → validate → EXPLAIN → budget check → execute → render
  render = first K rows, row count, column types, elapsed, calls used,
           tokens used, budget remaining, errors with hints
repeat until FINAL(...) or budget exhausted
```

- The root sees only the task and the catalog. The context is a table. The
  model peeks with `SELECT ... LIMIT`, partitions with `chunks`, greps with
  `grep`, and delegates with `RLM(...)`. These are exactly the emergent RLM
  strategies (peek, grep, partition-and-map, summarise) made explicit.
- Depth is capped at 2 by default. The reproduction study found depth 2 already
  degrades on current models, so depth is a dial, not a default.
- History is append-only (required for preserved-thinking on Claude Fable 5.1
  and cheaper for caching). Result renderings that grow stale are cleared with
  context editing rather than rewritten.
- Sub-calls run at `effort: low` on a cheaper model by default; the root runs
  at `high`. Both are `SET`-able per session and per call.

### 3.6 Continual harness and hard cases

The continual loop is itself a query over a `tasks` table: pull the next
pending task by curriculum policy, run a session, record outcome, cost and
difficulty, repeat. Generators keep the table non-empty:

| Generator | Hardness dial | Oracle |
|---|---|---|
| Random 3-SAT | clause/variable ratio near 4.26, n | code (`AS SHELL`) |
| Graph colouring / reachability / shortest path | n, edge density, k | code |
| Procedural puzzles (Reasoning Gym style: arithmetic chains, string rewriting, sudoku-like grids) | size / depth parameters | code |
| Long-context lookups over generated corpora (OOLONG-like) | corpus size, distractor rate | code |
| Repo questions over real checkouts | repo size, hop count | LLM judge with rubric |
| **Propose / Solve / Verify self-play** | learnability target (~50% solve rate, as in Absolute Zero Reasoner) | proposer writes task plus a `VERIFY` function; a separate verifier model or code oracle judges, never the proposer |

Curriculum: keep a running solve rate per (generator, difficulty); step the
dial up when solve rate exceeds 0.7, down below 0.3. The harness's own metrics
(calls, depth, branching, tokens, plan-space size) are recorded per task so
difficulty can be correlated with search complexity, which is the research
question.

### 3.7 Subagents

A subagent is a session with a **role**: its own system prompt, model, effort,
catalog subset (which tables and functions it may see), tool permissions and a
budget slice. `RLM(...)` is the anonymous case (same role as the parent, one
level deeper). Roles are declared in SQL and stored in the catalog, so the
model can define its own helpers and the harness can ship a standard set.

```sql
CREATE AGENT reviewer
  MODEL 'worker' EFFORT 'low'
  TOOLS (files, grep, lines)
  BUDGET (calls 40, tokens 60000)
  PROMPT 'You review one hypothesis against the code. Answer verdict, evidence.';

SELECT h.hypothesis, r.verdict, r.evidence
FROM hypotheses h
CROSS JOIN LATERAL SPAWN(reviewer, h.hypothesis, (SELECT * FROM ctx)) r
WHERE r.verdict = 'refuted';
```

Semantics:

- **Spawn returns rows, not chat.** A child session runs its own SQL REPL and
  ends with `FINAL`; the `FINAL` relation is what the parent's `SPAWN` yields.
  One child per input row, run concurrently under the parent's semaphore.
- **Handles and messages** for the asynchronous case, borrowed from Prime
  Agent: `SPAWN_ASYNC` returns a handle row at admission; children post to
  `INBOX` with `SEND(parent, text)`; the parent reads `INBOX` like any table
  and can `AWAIT(handle)` for the final relation. Most queries never need
  this; the synchronous `LATERAL SPAWN` is the default.
- **Budget slices.** A child gets an explicit slice of the parent's remaining
  budget, never more. Depth still counts: `SPAWN` at depth 2 is refused.
- **Catalog subsets** are the security boundary. A `reviewer` with
  `TOOLS (files, grep)` cannot call `shell` or `web_fetch`, and the planner
  knows this when estimating cost.
- **Cost attribution.** Child calls are rows in `trace_calls` with their own
  `session_id`; the parent's statement carries a rolled-up `child_cost` so
  both the tree and the flat view are one query away.

Standard roles shipped with the harness:

| Role | Used by | Model tier |
|---|---|---|
| `self` | `RLM(...)` | same as caller |
| `mapper` | partition-and-map over `chunks` or `files` | worker, low effort |
| `verifier` | `VERIFY`-style checks when no code oracle exists | worker, medium effort, separate from the proposer |
| `proposer` | continual harness task generation (3.6) | root tier |
| `judge` | rubric grading of `FINAL` answers for LLM-judged task families | worker |
| `planner` | optional: asks a model to choose between plan alternatives when the cost model is uncertain (3.3, rule 7) | worker |

In the algebra a spawn is ρ with a role parameter; its cost signature is the
child's plan, estimated from the role's budget when the child plan is not yet
known. The TUI shows children as a lane under their parent statement, with the
role name, and the tasks board shows proposer/solver/verifier triples as one
unit.

### 3.8 Model-agnostic LLM layer

Callgebra never talks to one vendor. `callgebra-llm` exposes a `Provider`
trait and a `Capabilities` struct (streaming, tools, structured output,
prompt caching, reasoning control, cost reporting) that the planner and the
harness consult instead of assuming.

**Backends, in order of preference:**

1. **Aura gateway** (`AuraProvider`, default when `AURA_BASE_URL` is set).
   Your Aura LLM Gateway speaks the Open Responses API over HTTP
   (`POST /v1/responses`, SSE with semantic events, `previous_response_id`,
   `usage.cost_usd`, `metadata.aura.provider/latency_ms`). Through it
   Callgebra gets nine providers, eight routing strategies with failover and
   circuit breaking, Redis response caching, per-request USD cost, rate limits
   and multi-tenancy without any of that code living here. Wire types come
   from the `aura-types` crate as a git dependency (it is light: serde, uuid,
   chrono, thiserror, utoipa), so the Open Responses structs are never
   hand-written. `aura-core` is **not** embedded: it drags in SQLx/Postgres,
   Redis and the AWS SDK. Two Aura features map directly onto Callgebra
   needs: `compression` (TOON/YAML) for rendering relations into prompts, and
   `consistency`/`validation` (self-consistency, best-of-N, confidence
   thresholds) as a physical implementation of `VERIFY`.
2. **Anthropic direct** (`AnthropicProvider`): Messages API over `reqwest`,
   for running without a gateway. Adds the knobs Open Responses does not carry
   today: adaptive thinking, `output_config.effort`, `cache_control`,
   structured outputs, `stop_reason: refusal`.
3. **OpenAI-compatible** (`OpenAiCompatProvider`): chat completions for vLLM,
   Ollama, and anything else local; also Aura's own `/v1` compatibility
   endpoint when Open Responses is not wanted.
4. **Mock / replay** (`ReplayProvider`, `RecordingProvider`): fixtures keyed by
   (model alias, prompt hash, schema hash), for tests and for free re-runs.

**Model aliases, not model names.** CallSQL and the planner refer to tiers:
`root`, `worker`, `proxy`, `judge`. `callgebra.toml` maps each alias to a
concrete model per backend, or, with Aura, to a routing goal
(`routing: { strategy: cost_optimized }`) so the gateway picks. Switching
vendors is a config change and a memo namespace change, nothing else.

**Provider-specific knobs** travel as `ProviderOptions`, a small enum the
provider either honours or reports as unsupported through `Capabilities`.
The harness degrades gracefully: without prompt caching it still freezes the
prefix (cheaper on every vendor); without structured output `LLM_JSON` falls
back to a single forced function tool and validates the arguments itself;
without reasoning control `EFFORT` is a no-op with a warning in the trace.

**Cost.** Actuals come from the provider when it reports them (Aura's
`usage.cost_usd`), otherwise from a local pricing table keyed by model. The
planner's estimates use the same table. Both are stored per call.

**Proposed upstream changes to Aura** (each is a small PR and lets Callgebra
drop a workaround): a `reasoning` field on `CreateResponseRequest` carrying
effort for providers that support it; a `text.format` / JSON-schema output
option; pass-through of Anthropic `cache_control` and reporting of
`cached_tokens` per request (the field exists on `Usage`); a feature-gated
`aura-providers` split of `aura-core::provider` with no database, Redis or
AWS dependency so the providers can be embedded in-process when no gateway
runs.

### 3.9 Tools and sandboxing

Filesystem tools are read-only by default and rooted at the session workspace.
`shell` runs inside `hakoniwa` (namespaces, tmpfs root, Landlock, seccomp,
cgroup limits) on Linux, with a plain-subprocess fallback flagged as unsafe.
`web_search`/`web_fetch` go through Anthropic's server-side tools when the
provider supports them, otherwise a pluggable HTTP backend with a domain
allow-list. Every tool call is a trace row with its arguments and byte counts.

### 3.10 Trace

A `tracing` `Layer` writes spans and events into SQLite: `runs`, `sessions`
(tree via `parent_id`, depth), `statements` (SQL, plan JSON, estimates,
actuals), `calls` (model, prompt hash, tokens in/out, cache read/write, dollars,
latency, parent statement, parent operator), `tool_calls`, `rounds` (for
recursion: round, delta size, frontier size). Child cost is attributed upward as
in Prime Agent's `child_usage_attributed` entries. The same tables are mounted
into every session's catalog as `trace_*` relations, so the model can ask
`SELECT SUM(dollars) FROM trace_calls WHERE session_id = current_session()`.

---

## 4. TUI

ratatui 0.30 + crossterm, tokio event loop (`EventStream` + `mpsc` in
`select!`), event-driven redraw, thin client over the daemon socket.

```
┌ callgebra ── session 3f2a · task: "find the bug in parser" · depth 0/2 ── $0.41 · cache 78% ┐
│ CALL TREE                  │ TRANSCRIPT                                   │ PLAN            │
│ ▾ session 3f2a  12 calls   │ ▸ turn 3                                     │ π answer        │
│   ▾ stmt 1  CREATE TABLE   │ ```sql                                       │ └ σ VERIFY(c)   │
│     λ LLM ×4   1.2k tok    │ WITH RECURSIVE frontier AS (                 │   λ  est 40     │
│   ▾ stmt 2  WITH RECURSIVE │   SELECT hypothesis, 0 AS d FROM seeds       │   act 37 ✓      │
│     μ round 0  frontier 4  │   UNION ALL                                  │   └ ⋉̸ REFUTE    │
│     μ round 1  frontier 11 │   SELECT e.item, d+1 FROM frontier           │     stopped @3  │
│     ▸ κ EXPAND ×11 ● run   │     CROSS JOIN LATERAL EXPAND(hypothesis,3) e│ fragment: REC   │
│     ρ RLM depth 1  ● run   │   WHERE d < 2 )                              │ plan space: 6   │
│   stmt 3  ...              │ SELECT ... WHERE VERIFY(hypothesis)          │─────────────────│
│                            │ ```                                          │ calls  ▁▂▃▅▇▆   │
│                            │ → 37 rows · 41 calls · 18.2k tok · 2.3s      │ tokens ▁▁▂▃▃▅   │
│                            │──────────────────────────────────────────────│ budget ████░ 80%│
│                            │ ● EXPAND(hypothesis="off-by-one in ...")     │ depth  ██░░░ 1/2│
│                            │ streaming: "1. The lexer advances past ..."  │ branch  2.75    │
├ 1 session  2 plan  3 trace  4 tasks  ?  help ── j/k move  ⏎ open  f fold  x cancel  q quit ┤
```

Views:

1. **Session**: call tree (left, `tui-tree-widget`, fold/unfold, live status
   dots, per-node tokens and dollars), transcript (centre, SQL highlighted
   with `syntect`, results rendered as tables, streaming pane for the selected
   call with `tui-markdown`), plan (right, `EXPLAIN` tree with estimate vs
   actual, fragment badge, plan-space size).
2. **Plan**: full-width operator tree; toggle between logical, call and
   physical views; the rewrite rules that fired; alternative plans considered
   with their costs (this is the "planning as search" view).
3. **Trace explorer**: a SQL prompt over the trace database inside the TUI,
   results as a scrollable table, with a few saved queries (cost by depth,
   calls per statement, cache hit rate over time).
4. **Tasks**: the continual harness board. Columns pending / running / solved /
   failed, grouped by generator and difficulty; a `Chart` of solve rate and
   cost per difficulty over time; the curriculum's current dial per generator.
5. **Help**: context-sensitive key bar (gitui style) plus a full keymap.

Details: sparklines and gauges for calls, tokens, budget and depth; compact
layout below 120×30 (crush); a theme with light and dark palettes; `Esc`
cancels the selected statement; `d` detaches (engine keeps running).

---

## 5. Workspace layout

```
callgebra/
├── Cargo.toml                 workspace, shared lints, release profile
├── crates/
│   ├── callgebra-sql/         parse, validate subset, Catalog, LogicalPlan
│   ├── callgebra-algebra/     CallPlan, CallKind, cost model, rules, EXPLAIN
│   ├── callgebra-exec/        async operators, recursion, budgets, memo
│   ├── callgebra-llm/         Provider trait, Aura (Open Responses), Anthropic, OpenAI-compat, replay
│   ├── callgebra-tools/       virtual tables, table functions, sandbox
│   ├── callgebra-trace/       event model, tracing Layer, SQLite schema
│   ├── callgebra-harness/     RLM REPL loop, sessions, agents and roles, continual loop, generators
│   ├── callgebra-daemon/      socket server, JSONL protocol, cursors
│   ├── callgebra-tui/         ratatui client
│   └── callgebra/             CLI binary: run, repl, explain, trace, bench, tui, daemon
├── tasks/                     task packs (yaml): prompt, context source, oracle
├── prompts/                   system prompt, catalog rendering, examples
├── tests/                     differential SQL tests, replayed sessions
└── docs/                      PLAN.md, RESEARCH.md, ADRs, dialect reference
```

Dependencies (versions verified on crates.io on 2026-09-11): `sqlparser 0.62`,
`tokio 1.53`, `reqwest` (rustls), `serde`/`serde_json`, `rusqlite 0.40`
(bundled), `tracing 0.1` / `tracing-subscriber 0.3`, `ratatui 0.30.2`,
`crossterm 0.29`, `tui-tree-widget 0.24`, `tui-markdown 0.3`, `syntect`,
`hakoniwa 1.7`, `clap`, `anyhow`/`thiserror`, `proptest`, `insta`, and
`aura-types` as a git dependency on `UmaiTech/aura-llm-gateway` (0.18, MIT).
Optional later: `ascent`/`datafrog` (not needed, semi-naive is small to write),
`duckdb` (Arrow-native analytics over big traces), `wasmtime` (WASM tool
plugins), `opentelemetry` export.

---

## 6. Milestones

Each milestone ends with something runnable and a demo query.

**M0 — Scaffold (small).** Workspace, CI (fmt, clippy, test), `MockProvider`,
`tracing` to stderr, `callgebra --version`. README with the pitch.

**M1 — Relational core.** `callgebra-sql` + `callgebra-exec` for the pure
subset: SELECT/JOIN/WHERE/GROUP BY/ORDER/LIMIT, subqueries, EXISTS, CTEs and
recursive CTEs with semi-naive evaluation. Differential tests against SQLite
with `proptest`-generated queries and data. Demo: transitive closure and
shortest path over a generated graph, `callgebra repl` with a local table.

**M2 — Call algebra.** `LLM*` scalar functions, prompt-defined
`CREATE FUNCTION`, `EXPAND`, `LATERAL` table functions, filesystem tools.
`CallPlan` with `CallKind`, dedupe-then-map, batching, memo in SQLite,
budgets, `EXPLAIN` with estimates. `Provider` trait with the Aura backend
(Open Responses over HTTP, streaming, cost from `usage.cost_usd`), the
Anthropic direct backend, and recording/replay fixtures; model aliases in
`callgebra.toml`. Demo: the `VERIFY / NOT EXISTS REFUTE`
query from the pitch over a generated candidate set, with `EXPLAIN ANALYZE`
showing 40 estimated vs 37 actual calls and the anti-semi-join short-circuit.

**M3 — RLM harness.** Session model, SQL REPL loop, catalog rendering in the
cached system prompt, result truncation, `FINAL`, `RLM(...)` recursion with
depth and budget inheritance, `CREATE AGENT` roles and `LATERAL SPAWN`,
session persistence and `resume`. Demo: a
long-context question over a generated corpus (OOLONG-like) where the root
partitions with `chunks` and maps `RLM` over the partitions; and a repo
question over this repository where the root spawns `reviewer` agents over
candidate hypotheses.

**M4 — TUI v1.** Daemon + JSONL protocol, session view (tree, transcript,
streaming pane, plan sidebar), status bar, cancel, detach/reattach, headless
mode. Demo: watch M3's demo run live, fold the tree, cancel a statement.

**M5 — Planner.** Cost model with sampled selectivity for call predicates,
cheap-first, semi-join, cascade with proxy thresholds, beam-limited recursion,
join-order enumeration over call predicates with a memo and branch-and-bound.
Plan view showing alternatives and the size of the plan space. Demo: a
three-way join with two LLM predicates where the naive plan costs 10× the
chosen one, and a query whose plan enumeration is visibly exponential.

**M6 — Continual harness.** `tasks` table, generators (3-SAT, graph problems,
procedural puzzles, generated corpora, repo questions), propose/solve/verify
with a separate verifier, curriculum, task board view, unattended run with
budgets and resume. Demo: overnight run, morning `SELECT generator,
difficulty, AVG(solved), AVG(calls), AVG(depth) FROM trace_tasks GROUP BY 1, 2`.

**M7 — Research artefacts.** Benchmark scripts, plots (accuracy vs context,
cost vs baseline, calls vs difficulty, plan-space size vs query shape), the
dialect reference, and a write-up: "Callgebra: a relational calculus for
language-model computation".

Suggested order of attack after this plan is approved: M0 and M1 in one go,
since M1 is deterministic and testable without any API key.

---

## 7. Risks and how the plan handles them

| Risk | Mitigation |
|---|---|
| Model writes invalid or unsupported SQL | Strict subset, parse and validation errors returned as result rows with hints and the nearest supported form; examples in the cached prefix; `EXPLAIN` before every execution. |
| Cost blow-up from a careless `CROSS JOIN` with a call predicate | Mandatory `EXPLAIN` estimate and budget refusal before execution; per-statement and per-session budgets; dedupe and memo; hard cap on calls per statement. |
| Deep recursion degrades quality (observed at depth 2 on current models) | Depth capped at 2, default 1 for sub-calls; branching and frontier limits; budget slices shrink with depth. |
| Prompt cache misses from a changing prefix | Catalog and rules frozen per session, one breakpoint, append-only history, cache read tokens visible in the TUI. |
| Non-determinism makes tests flaky | `MockProvider` replay of recorded fixtures; all relational tests are differential against SQLite and need no model at all. |
| Sandbox escape via `shell` | `hakoniwa` on Linux, read-only workspace by default, explicit allow-list for network, every call traced. Unsafe fallback must be opted into. |
| Name collision | `callgebra` exists as a small JavaScript library (`fluture-js/callgebra`) and npm package; crates.io and PyPI are free. Keep the name, note the prior art in the README. |
| Scope creep | The milestones are cumulative and each ends with a demo; the planner (M5) and continual harness (M6) come after a working RLM loop, not before. |

---

## 8. Open questions for you

1. **Aura as the default path.** The plan makes Aura the preferred backend
   and keeps the direct Anthropic and OpenAI-compatible backends for
   gateway-less runs. Should Aura be *required* instead (one code path, all
   routing in the gateway), and are you open to the small upstream additions
   listed in 3.8 (reasoning effort, JSON-schema output, cache pass-through,
   an embeddable `aura-providers` crate)?
2. **First target task family for M3.** Generated long-context corpus (fully
   offline, deterministic oracle) or repository questions (more compelling demo,
   LLM-judged)? The plan assumes both, corpus first.
3. **Web search backend.** Anthropic server-side `web_search` (simplest) or a
   pluggable backend for local models too?
4. **Trace store.** SQLite only (chosen) or SQLite plus DuckDB for analytics
   over large traces?

---

## 9. How it gets built

The crates in section 5 have thin, explicit interfaces, which is deliberate:
implementation fans out to parallel subagents, one per crate, with the
interfaces fixed first.

1. **Interfaces first (one agent, short).** `Value`, `Batch`, `LogicalPlan`,
   `CallPlan`, `Provider`, `Tool`, the trace event enum and the daemon
   protocol as `.rs` files with doc comments and `todo!()` bodies, committed
   before any implementation. This is the contract every other agent codes
   against.
2. **M0 + M1 in parallel.** Agents for `callgebra-sql` (parser subset and
   planner), `callgebra-exec` (operators and semi-naive recursion),
   `callgebra-trace` (schema and layer) and the differential test harness
   against SQLite. Integration and review by the coordinating session.
3. **M2 in parallel.** `callgebra-llm` (Aura, Anthropic, replay),
   `callgebra-algebra` (call kinds, rules, EXPLAIN), `callgebra-tools`.
4. **M3 onward** alternates: harness and agents, then TUI and daemon, then
   planner, then generators. Each milestone ends with a code review pass and
   the demo query recorded as a replay fixture so it runs in CI for free.

Every agent works on its own branch off `claude/callgebra-sql-rlm-9opg46`,
with `cargo fmt`, `cargo clippy -D warnings` and `cargo test` green before
merge. The coordinating session owns the workspace `Cargo.toml`, the
interface crate and the merge order.
