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
   DuckDB tables. The model, the harness and the human all inspect execution with the
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
        Anthropic direct web, sandbox)      with a role, budget slice
        OpenAI-compatible                   and catalog subset
        Open Responses gateway (optional, e.g. Aura)
        replay / record
       alias router, pricing, failover
               │               │               │
               └───────────────┴───────────────┘
                               ▼
        ┌──────────────── callgebra-store (DuckDB) ────┐
        │ session tables · memo · append-only trace    │
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
| DuckDB (embedded, `duckdb` crate) | Used as **store and oracle**, not as engine | Its UDFs are synchronous, so calls would serialise, and there is no plan-level control. But it is one embedded file for session tables, memo and trace, it is Arrow-native and fast at analytics over large traces, and its SQL coverage (recursive CTEs, LATERAL, list functions) makes it the reference semantics for differential tests. Trade-off: the bundled build adds minutes to a clean compile; CI caches it and `DUCKDB_LIB_DIR` can point at a prebuilt library. |
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

#### Everything the model does is SQL

There is no second channel. The model has no tool-calling API, no free-text
commands, no Python. Every action, including side effects, is a CallSQL
statement, so every action is planned, budgeted, traced and replayable the
same way. The tool surface is therefore a catalog of relations and functions,
each tagged with a **volatility class** the planner respects (the Postgres
terms, with the same meaning):

| Class | Meaning | Planner may |
|---|---|---|
| `IMMUTABLE` | same inputs, same output, forever (`chunks`, `SIM`; `LLM` with a pinned model is *treated* as immutable for memo purposes and flagged as such) | reorder, dedupe, memoise across runs |
| `STABLE` | constant within one statement (`files`, `lines`, `grep`, `git_log`, `INBOX`) | reorder, dedupe within the statement |
| `VOLATILE` | side effects or non-repeatable (`shell`, `write_file`, `web_fetch`, `SEND`) | nothing: evaluated exactly once per input row, in input order, never memoised, never moved past another volatile |

Read surface (STABLE unless noted):

| Relation / function | Columns or result |
|---|---|
| `files(glob)` | `path, size, mtime, kind` |
| `lines(path)` | `lineno, text` |
| `grep(pattern [, glob])` | `path, lineno, text` |
| `read(path)` | `text` (one row); parses `.docx`, `.pdf`, `.xlsx`, `.pptx` as well as text, via `pandoc` in the sandbox with Rust readers as fallback |
| `chunks(text, size [, overlap])` IMMUTABLE | `ordinal, text, tokens` |
| `git_log([n])`, `git_diff([ref])`, `git_blame(path)` | commit and line-level history |
| `env(name)` | value, allow-listed names only |
| `INBOX` | `from_session, ts, text`, messages from children (3.7) |
| `memory` | `key, value, ts, session_id`, a plain table that persists across runs |
| `trace_calls`, `trace_stmts`, `trace_sessions`, `trace_rounds` | the run's own trace, live |
| `playbook` | learned SQL snippets and their success stats (3.6) |

Write and side-effect surface (all VOLATILE, all statements rather than
expressions so they cannot hide inside a predicate):

| Statement | Result relation |
|---|---|
| `CALL shell(cmd [, cwd, timeout])` | `stdout, stderr, exit_code, duration_ms`; long output is also written to `shell_output(run_id, lineno, stream, text)` so the model can `grep` it instead of paging it into context |
| `CALL write_file(path, text)`, `CALL append_file(path, text)` | `path, bytes` |
| `CALL patch(path, old, new)` | `path, replaced`; rejected if the file changed since the model last read it, the staleness check a dedicated operator can enforce and a shell string cannot |
| `CALL mkdir(path)`, `CALL remove(path)` | `path` |
| `CALL web_search(q [, n])` | `rank, title, url, snippet` |
| `CALL web_fetch(url)` | `url, status, text, tokens` |
| `CALL git_commit(message)` | `sha` |
| `INSERT INTO memory ...` | ordinary DML on the persistent table |
| `SEND(handle, text)` | delivery row |

Row-wise side effects go through `CALL ... FROM`:
`CALL write_file(path, new_text) FROM rewritten_files;` runs once per row, in
order. Because the arguments are a relation, the model naturally batches
edits, and the trace records one statement with n effects rather than n
opaque commands.

Permissions are per session and per agent role (3.7): a role lists the
relations and `CALL`s it may use, write paths are allow-listed, and `shell`
runs in the sandbox (3.9). A statement that touches something outside the
role's surface fails at validation, before planning, with the same
error-as-result-row treatment as a syntax error.

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
8. **Volatility fences**: rules 1 to 7 apply only across `IMMUTABLE` and
   `STABLE` operators. A `VOLATILE` operator is a fence: nothing moves across
   it, it is never deduplicated, and its input order is preserved.

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
- **Memo**: DuckDB table `memo(model, prompt_hash, schema_hash, output, usage)`,
  shared across sessions and runs. Replaying a run against a warm memo is free,
  which is how the test-suite runs real prompts without real calls.
- **Session state**: every `CREATE TABLE` lands in the run's DuckDB file.
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

### 3.6 Continual, self-learning harness

The continual loop is itself a query over a `tasks` table: pull the next
pending task by curriculum policy, run a session, record outcome, cost and
difficulty, repeat. Two things keep it alive: generators that keep the table
non-empty, and learning that changes how the next task is attempted.

#### Where tasks come from

| Source | Hardness dial | Oracle |
|---|---|---|
| **User input**: tasks typed in the TUI or dropped into `tasks/` as YAML, plus every real question asked in an interactive session (recorded as a task with the user's accept/reject as the label) | as given; difficulty is *estimated* (below) | user verdict, or a user-supplied check |
| Random 3-SAT | clause/variable ratio near 4.26, n | code (`AS SHELL`) |
| Graph colouring / reachability / shortest path | n, edge density, k | code |
| Procedural puzzles (Reasoning Gym style) | size, depth | code |
| Long-context lookups over generated corpora (OOLONG-like) | corpus size, distractor rate | code |
| Repo questions and edits over real checkouts | repo size, hop count, tests to pass | tests, or LLM judge with rubric |
| **Propose / Solve / Verify self-play** | learnability target (~50% solve rate, as in Absolute Zero Reasoner) | proposer writes task plus a `VERIFY` function; a separate verifier agent or code oracle judges, never the proposer |

#### Difficulty

Every task and every solver configuration (model alias, effort, depth cap,
budget) gets a rating. Solve/fail outcomes update both with a Bradley-Terry
style pairwise model: a task that beats strong configurations is hard, a
configuration that beats hard tasks is strong. Ratings live in
`task_ratings` and `solver_ratings`, so "how hard is this" and "which
configuration should try it" are both `ORDER BY rating` queries. Generated
tasks carry their generator's dial as a prior; user tasks get a prior from a
cheap classifier call, refined by outcomes.

Curriculum policy: sample the next task where the current solver's expected
solve probability is near 0.5, step a generator's dial up when its solve rate
exceeds 0.7 and down below 0.3, and re-queue failed user tasks after the
harness has learned something relevant (a new playbook entry or a changed
function).

#### What the harness learns (no weight updates)

All of it lives in tables, all of it is inspectable and revertible, and every
change is adopted only after it wins on a replay eval (section 7).

| Learned object | Table | Signal | Used by |
|---|---|---|---|
| **Playbook**: SQL snippets that solved tasks, keyed by task kind and the catalog they needed | `playbook(kind, sql, wins, tries, avg_cost)` | `FINAL` accepted by oracle or user | rendered into the cached prefix as examples for similar tasks; queryable by the model (`SELECT sql FROM playbook WHERE kind = ...`) |
| **Prompt-defined functions**: refined prompts for `VERIFY`, `mapper`, `judge` roles | `functions` with a version ledger | disagreement with oracles, user corrections | catalog; Prime Agent style `/refine` with before/after snapshots and rollback |
| **Cost model**: selectivity and branching estimates per predicate template and per `EXPAND` prompt | `estimates(template_hash, sel, branch, n)` | `EXPLAIN ANALYZE` actuals | planner (3.3); a learned optimizer in the classical sense |
| **Routing**: which alias solved which task kinds at what cost | `solver_ratings` | outcomes | default alias per role and per task kind; with Aura, exported as routing weights |
| **Curriculum state** | `generator_state(generator, dial, solve_rate)` | outcomes | task selection |
| **User preferences**: accepted answer shapes, verbosity, forbidden actions | `preferences` | explicit thumbs, edits to `FINAL`, rejections | rendered into the prefix; enforced as role permissions where they are hard rules |

Guardrails: the base system prompt is immutable; learned content is appended
in clearly marked sections; every learned object has a version and a
`learned_from` trace pointer; adoption requires beating the current version
on the replay eval for the affected task kinds at equal or lower cost; a
`callgebra learn revert <version>` command exists and is tested.

#### Weight-level learning (stretch)

Traces already have the shape of rollouts: a tree of sessions, statements,
calls and rewards. An exporter writes them in the `verifiers` trace format
so a Callgebra task pack can be a `prime-rl` environment, with `VERIFY`
functions as the reward. Out of scope until the harness-level learning above
has data to show it is worth it.

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

Callgebra never talks to one vendor, and it does not need a gateway to be
model-agnostic. The surface it needs from a model is narrow: one request
shape (system prefix, messages, optional tools, optional JSON schema for the
output), streaming, usage. So the layer is small and owned here:

```
Provider trait  ─┬─ AnthropicProvider   Messages API, native (thinking, effort,
                 │                      cache_control, structured output, refusal)
                 ├─ OpenAiCompatProvider chat completions: OpenAI, Google, Mistral,
                 │                      Together, Fireworks, Ollama, vLLM, and any
                 │                      gateway's /v1 endpoint
                 ├─ OpenResponsesProvider (feature "gateway") Aura or any Open
                 │                      Responses gateway; cost from usage.cost_usd
                 └─ ReplayProvider / RecordingProvider  fixtures for tests and re-runs
Router           alias → ordered (backend, model) candidates, failover, per-tier
                 budgets, pricing table, health per endpoint
Capabilities     streaming · tools · json_schema · prompt_cache · reasoning_control ·
                 cost_reported   (per backend; the planner and harness consult it)
```

**Lightweight by default.** Two direct adapters cover almost every vendor:
Anthropic's native API, and the OpenAI-compatible chat API that everyone
else, including local servers, exposes. Each is a few hundred lines over
`reqwest` and an SSE decoder. No SDK, no gateway, no database.

**What is copied from Aura** rather than depended on: the shape of its
fallback chain (ordered candidates, circuit breaker per endpoint), its model
catalog and cost calculator (a pricing table keyed by model, actuals computed
from usage), and the idea that the internal message model is vendor-neutral
and each adapter maps to and from the wire format. Those are small modules
and they belong in `callgebra-llm`; pulling `aura-core` would bring
SQLx/Postgres, Redis and the AWS SDK with it.

**Aura as an option, not a requirement.** With the `gateway` feature the
`OpenResponsesProvider` speaks the Open Responses API (`POST /v1/responses`,
semantic SSE events, `usage.cost_usd`, provider and latency metadata), and
Aura's routing, caching, rate limits and multi-tenancy sit behind one URL.
Aura's `compression` (TOON/YAML for rendering relations into prompts) and
`consistency`/`validation` (best-of-N as a physical implementation of
`VERIFY`) are then available as `ProviderOptions`. Any other Open Responses
or OpenAI-compatible gateway (LiteLLM, Portkey, OpenRouter) works through the
same two adapters with no new code.

**Aura is ours to change.** Aura is Marcus's project, so a gap the gateway
adapter hits is filed as an issue on `UmaiTech/aura-llm-gateway` and fixed
there, not papered over here. The four gaps already known are drafted in
`docs/aura-issues/` (reasoning effort on the request, JSON-schema output,
cache-control pass-through with `cached_tokens` reported, an embeddable
`aura-providers` crate). The adapter still degrades gracefully in the
meantime, per the `Capabilities` rules below.

**Alternatives considered.** The `genai` crate (0.6.5, 26+ providers, native
Anthropic protocol, cache control, reasoning effort, structured output) would
replace both direct adapters in a day. It is the fallback if the adapters
turn into a maintenance burden, and the reason the `Provider` trait is ours:
swapping the implementation must not touch the planner or the harness.
`rig-core` is a whole agent framework and is more than we want.

**Model aliases, not model names.** CallSQL and the planner refer to tiers:
`root`, `worker`, `proxy`, `judge`. `callgebra.toml` maps each alias to an
ordered list of `(backend, model)` candidates; the router tries them in
order and records which one served. Switching vendors is a config change and
a memo namespace change, nothing else.

**Provider-specific knobs** travel as `ProviderOptions`, a small enum the
adapter either honours or reports as unsupported through `Capabilities`.
The harness degrades gracefully: without prompt caching it still freezes the
prefix (cheaper on every vendor); without JSON-schema output `LLM_JSON`
falls back to a single forced function tool and validates the arguments
itself; without reasoning control `EFFORT` is a no-op with a warning in the
trace.

**Cost.** Actuals come from the provider when it reports them (a gateway's
`usage.cost_usd`), otherwise from the local pricing table. The planner's
estimates use the same table. Both are stored per call.

### 3.9 Tools and sandboxing

Filesystem tools are read-only by default and rooted at the session workspace.
`shell` runs inside `hakoniwa` (namespaces, tmpfs root, Landlock, seccomp,
cgroup limits) on Linux, with a plain-subprocess fallback flagged as unsafe.
`web_search`/`web_fetch` go through Anthropic's server-side tools when the
provider supports them, otherwise a pluggable HTTP backend with a domain
allow-list. Every tool call is a trace row with its arguments and byte counts.

### 3.10 Trace

A `tracing` `Layer` writes spans and events into DuckDB through the appender
API: `runs`, `sessions`
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
│   ├── callgebra-llm/         Provider trait, Anthropic, OpenAI-compat, Open Responses (feature), router, pricing, replay
│   ├── callgebra-tools/       virtual tables, table functions, sandbox
│   ├── callgebra-store/       DuckDB: session tables, memo, trace schema, appender
│   ├── callgebra-trace/       event model, tracing Layer → store
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
`tokio 1.53`, `reqwest` (rustls), `serde`/`serde_json`, `duckdb 1.10505`
(bundled, DuckDB 1.5), `tracing 0.1` / `tracing-subscriber 0.3`, `ratatui 0.30.2`,
`crossterm 0.29`, `tui-tree-widget 0.24`, `tui-markdown 0.3`, `syntect`,
`hakoniwa 1.7`, `clap`, `anyhow`/`thiserror`, `proptest`, `insta`. No SDK
and no gateway dependency: the two direct adapters are written here.
Optional later: `ascent`/`datafrog` (not needed, semi-naive is small to write),
`wasmtime` (WASM tool plugins), `opentelemetry` export.

---

## 6. Milestones

Each milestone ends with something runnable and a demo query.

**M0 — Scaffold (small).** Workspace, CI (fmt, clippy, test), `MockProvider`,
`tracing` to stderr, `callgebra --version`. README with the pitch.

**M1 — Relational core.** `callgebra-sql` + `callgebra-exec` for the pure
subset: SELECT/JOIN/WHERE/GROUP BY/ORDER/LIMIT, subqueries, EXISTS, CTEs and
recursive CTEs with semi-naive evaluation. DuckDB store. Differential tests
against DuckDB
with `proptest`-generated queries and data. Demo: transitive closure and
shortest path over a generated graph, `callgebra repl` with a local table.

**M2 — Call algebra.** `LLM*` scalar functions, prompt-defined
`CREATE FUNCTION`, `EXPAND`, `LATERAL` table functions, the read tool
surface and the `CALL` side-effect statements with volatility fences.
`CallPlan` with `CallKind`, dedupe-then-map, batching, memo in DuckDB,
budgets, `EXPLAIN` with estimates. `Provider` trait with the Anthropic and
OpenAI-compatible adapters, the alias router with failover and pricing,
recording/replay fixtures, and the optional Open Responses gateway adapter
behind a feature flag. Demo: the `VERIFY / NOT EXISTS REFUTE`
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

**M6 — Continual, self-learning harness.** `tasks` table, user-submitted
tasks, generators (3-SAT, graph problems, procedural puzzles, generated
corpora, repo questions), propose/solve/verify with a separate verifier,
ratings and curriculum, playbook and function refinement with replay-gated
adoption and revert, task board view, unattended run with budgets and
resume. Demo: overnight run, morning `SELECT generator,
difficulty, AVG(solved), AVG(calls), AVG(depth) FROM trace_tasks GROUP BY 1, 2`.

**M7 — Benchmarks and write-up.** Task packs for OOLONG and a Terminal-Bench
subset first, then Harvey LAB scored with its own evaluator, then the
finance and remaining legal learning tracks with their
learning-on versus learning-off runs, then the rest of section 7; the plain-agent baseline on the
same provider layer; plots (accuracy vs context at cost parity, calls vs
difficulty, estimate accuracy over time, plan-space size vs query shape);
the dialect reference; and a write-up: "Callgebra: a relational calculus for
language-model computation".

Suggested order of attack after this plan is approved: M0 and M1 in one go,
since M1 is deterministic and testable without any API key.

---

## 7. Benchmarks and evals

Three layers, cheapest first. The first two run in CI without a model.

**Correctness of the engine (no model).**
- Differential tests: `proptest` generates schemas, data and CallSQL queries
  in the pure subset; results must match DuckDB row for row, recursive CTEs
  included.
- Planner invariants: every rewrite rule has a property test that the
  rewritten plan's result equals the original's on the replay provider.
- Replay evals: every demo query and every solved task is recorded as a
  fixture, so the whole task suite re-runs offline; a diff in `FINAL` or in
  call count is a regression.

**Efficiency of the planner (model optional).**
- Estimate accuracy: estimated versus actual calls, tokens and dollars per
  statement, tracked over time; the learned cost model (3.6) must move this
  toward zero.
- Cost parity, the RLM paper's framing: for each task family, accuracy of
  Callgebra against a plain tool-calling agent loop *built on the same
  provider layer and budgets*, so the comparison isolates the SQL abstraction
  rather than the model or the client, at equal spend.
- Plan-space size and planning time per query shape, the "where planning
  becomes search" curve.

**External benchmarks (model required).** Start with the two marked first;
the rest become task packs under `tasks/` once the harness is stable.

| Benchmark | What it exercises here | Notes |
|---|---|---|
| **OOLONG** and OOLONG-Pairs (first) | long-context aggregation and pairwise reasoning; partition-and-map with `chunks` and `RLM`; the headline RLM comparison | offline, deterministic scoring; the RLM paper's numbers are the baseline to beat at cost parity |
| **BrowseComp-Plus** | retrieval over a fixed corpus of 10 to 1,000 documents; `grep` and semantic filters; `VERIFY` on candidate answers | fixed corpus, no live web needed |
| **Terminal-Bench** (first, subset) | the whole SQL-mapped shell surface: `CALL shell`, `patch`, `write_file`, tests as oracles | proves the "everything is SQL" thesis on real terminal tasks |
| **SWE-bench Verified / Lite** (subset) | repo navigation with `files`/`grep`/`lines`, hypothesis search with `EXPAND` and `reviewer` agents, edits with `patch`, tests as `VERIFY` | expensive; a 50-task subset is enough to compare plans |
| **Spider 2.0 / BIRD** | the model's ability to write correct SQL under a schema, a prerequisite for everything else | cheap; also measures the CallSQL frontend's error messages |
| **LongBench-v2**, RULER, S-NIAH | needle and reasoning over long inputs; depth 1 versus depth 2 | RULER's synthetic tasks double as generators |
| **Reasoning Gym** | procedural reasoning with verifiers; the same generators feed the continual harness | offline, unlimited instances |
| Random 3-SAT near 4.26, **GraphOmni** / NLGraph, **ZebraLogic** | recursive-CTE search with `EXPAND`, beam limits, `VERIFY`/`REFUTE` structure | code oracles throughout |
| **SemBench**, TAG-Bench | semantic-operator systems: cost and quality of semantic filters, joins and aggregations against LOTUS and Palimpzest | the direct comparison for the planner's cascades and semi-joins |

**Domain tracks: does self-learning work?** General benchmarks measure a
single attempt. To measure *learning*, a track needs many related tasks in
one domain, real documents that reward reusable extraction logic, and cheap
oracles. Legal and finance both qualify, and both stress the SQL surface
(long filings and contracts go through `chunks`, `grep`, extraction into
tables, and aggregation).

| Track | Dataset | Tasks and oracle | Why it tests learning |
|---|---|---|---|
| **Finance** | FinanceBench (150 open questions over 10-K/10-Q filings; full set on request from Patronus) | numeric and factual answers with evidence pages; numeric tolerance plus evidence-page match | the same filing structure recurs across companies, so a learned `find_line_item(kind)` function and playbook entries ("income statement chunk, then grep the label") should transfer |
| **Finance** | FinQA (numerical reasoning over report pages, MIT) and TAT-QA (16,552 questions over hybrid table plus text contexts) | program or number exact match | thousands of instances, so learning curves have statistical power; questions cluster by reasoning type (ratio, growth, sum) |
| **Legal** | CUAD (510 commercial contracts, 41 clause categories, CC BY 4.0) | clause spans per category; token F1 against gold spans | one `CREATE FUNCTION has_clause(kind, text)` per category is refined contract after contract; measure per-category precision and recall over the stream |
| **Legal, headline** | **Harvey LAB** (Legal Agent Benchmark, MIT, `harveyai/harvey-labs`): 1,671 tasks across 24 practice areas plus contracting, each a `task.json` (instructions, `work_type` analyze / draft / review / research, expected deliverable files) with a `documents/` matter folder; graded against 75,000+ expert-written pass/fail criteria by two LLM judges (defaults Claude Sonnet 4.6 and GPT-5.5) with **all-pass** scoring, so a task counts only if every criterion passes. Frontier models completed under 10% end to end at launch (May 2026); the Artificial Analysis leaderboard sits around 25% as of September 2026 | LAB's own evaluator, unchanged: Callgebra writes deliverables to `output/` and `evaluation.run_eval` scores them, so numbers are comparable with the public leaderboard. All-pass rate is the headline, criterion pass rate the diagnostic | LAB's agent gets exactly `bash, read, write, edit, glob, grep, finish`, which is the CallSQL surface (`shell`, `read`, `write_file`, `patch`, `files`, `grep`, `FINAL`) one to one, so it is the cleanest external test of "everything is SQL". Workflows repeat across scenarios (`extract-psa-key-terms/scenario-01..`), the shape self-learning needs; the harsh all-pass metric makes any learning gain unambiguous |
| **Legal** | Harvey's BigLaw Bench (Core: transactional and litigation tasks; Workflows: SPA Deal Points extraction across Share Purchase Agreements; Retrieval: long contracts with cross-references and defined terms, plus discovery emails). Samples and rubrics public, full set on request from Harvey | rubric grading with positive points for requirements met and negative points for errors such as hallucinations, plus source reliability; scored by a `judge` role against the published rubrics, with the answer score read as "% of a lawyer-quality work product" | Workflows repeats one extraction schema across many SPAs, the ideal shape for refined `CREATE FUNCTION` extractors and playbook transfer; Retrieval's cross-references and defined terms exercise `chunks`, `grep` and recursive CTEs (follow a defined term to its definition) |
| **Legal** | LegalBench (162 tasks; per-task licenses, filter to permissive) | mostly yes/no and short-answer, exact match | many small task families with shared legal reasoning; tests whether playbook entries transfer *across* tasks, not just within |
| **Synthetic, both** | generated contracts with planted clauses; generated statements with planted figures and inconsistencies | code oracle, unlimited instances | feeds the continual generators; hardness dialled by document length, distractors and paraphrase |

Measurement design, the same for every track:

1. **Learning curve.** Run the task stream in a fixed order with learning on.
   Plot accuracy and cost per task as a rolling mean. The control is the same
   stream with learning off (frozen catalog, no playbook). The claim is the
   gap, not the absolute number.
2. **Transfer.** Learn on the first part of the stream (contracts 1 to 300,
   filings of half the companies), evaluate frozen on the rest. Report the
   delta against the never-learned harness on the held-out part.
3. **Cost.** Learned playbooks should reduce calls and tokens per task, not
   only raise accuracy; both go in the `evals` table.
4. **Damage control.** Every learned object that was adopted is tied to the
   replay eval that admitted it; the report lists reverts, so the learning
   rate and the regret are both visible.

Running LAB from Callgebra: import each task directory as a task row with
the matter folder mounted read-only as the workspace; `read(path)` must
parse `.docx`, `.pdf`, `.xlsx` and `.pptx` (LAB uses Pandoc, MarkItDown and
pdfplumber; Callgebra shells out to `pandoc` in the sandbox and falls back
to Rust readers); deliverables are written with `CALL write_file`, `.docx`
outputs via `pandoc` from Markdown; `FINAL` lists the deliverables the way
LAB's `finish` tool does. Then LAB's `evaluation.run_eval` scores the
`output/` directory. Runs go in `results/<run-id>/` in LAB's layout, so its
comparison dashboards work on Callgebra runs too. Harvey also publishes
BigLaw Bench: Research for agentic legal research; it joins the track once
LAB runs.

Licensing note: Harvey LAB is MIT; FinanceBench's open sample and CUAD are permissive;
LegalBench is mixed and must be filtered per task; FinQA is MIT; TAT-QA's
terms should be checked before redistribution of derived fixtures.

Reporting: one `evals` table per run with accuracy, calls, tokens, dollars,
depth, branching and wall clock per task; a `callgebra bench` command that
renders it; the TUI tasks board reads the same table. Baselines are recorded
as replay fixtures too, so a comparison is reproducible without re-spending.

## 8. Risks and how the plan handles them

| Risk | Mitigation |
|---|---|
| Model writes invalid or unsupported SQL | Strict subset, parse and validation errors returned as result rows with hints and the nearest supported form; examples in the cached prefix; `EXPLAIN` before every execution. |
| Cost blow-up from a careless `CROSS JOIN` with a call predicate | Mandatory `EXPLAIN` estimate and budget refusal before execution; per-statement and per-session budgets; dedupe and memo; hard cap on calls per statement. |
| Deep recursion degrades quality (observed at depth 2 on current models) | Depth capped at 2, default 1 for sub-calls; branching and frontier limits; budget slices shrink with depth. |
| Prompt cache misses from a changing prefix | Catalog and rules frozen per session, one breakpoint, append-only history, cache read tokens visible in the TUI. |
| Non-determinism makes tests flaky | `ReplayProvider` fixtures; all relational tests are differential against DuckDB and need no model at all. |
| DuckDB bundled build slows clean compiles | Cached in CI, `DUCKDB_LIB_DIR` for a prebuilt library locally, and the store sits behind one crate so nothing else recompiles when it does. |
| Sandbox escape via `shell` | `hakoniwa` on Linux, read-only workspace by default, explicit allow-list for network, every call traced. Unsafe fallback must be opted into. |
| Name collision | `callgebra` exists as a small JavaScript library (`fluture-js/callgebra`) and npm package; crates.io and PyPI are free. Keep the name, note the prior art in the README. |
| Scope creep | The milestones are cumulative and each ends with a demo; the planner (M5) and continual harness (M6) come after a working RLM loop, not before. |

---

## 9. Open questions for you

Resolved so far: the store is DuckDB, and every action the model takes is SQL
(no side channel for tools).

1. **Provider layer.** The plan now writes two thin adapters (Anthropic,
   OpenAI-compatible) plus an optional Open Responses gateway adapter, and
   copies Aura's fallback-chain and pricing shapes rather than depending on
   it. If you would rather not maintain adapters at all, the `genai` crate
   behind the same trait is the one-day alternative. Which do you prefer?
2. **First target task family for M3.** Generated long-context corpus (fully
   offline, deterministic oracle) or repository questions (more compelling demo,
   LLM-judged)? The plan assumes both, corpus first.
3. **Web search backend.** Anthropic server-side `web_search` (simplest) or a
   pluggable backend for local models too?
4. **First benchmark.** Section 7 lists candidates. Which one should the
   M3 demo be built around: an OOLONG-style long-context task (offline,
   cheap) or a Terminal-Bench-style task pack (exercises the SQL-mapped shell
   surface)?
5. **Domain track order.** Finance first (FinanceBench and FinQA have
   numeric oracles and thousands of instances) or legal first (Harvey LAB is
   the strongest public signal, and its tool surface matches ours exactly)?
   The plan now assumes legal first with LAB, finance second.

---

## 10. How it gets built

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
   against DuckDB. Integration and review by the coordinating session.
3. **M2 in parallel.** `callgebra-llm` (Aura, Anthropic, replay),
   `callgebra-algebra` (call kinds, rules, EXPLAIN), `callgebra-tools`.
4. **M3 onward** alternates: harness and agents, then TUI and daemon, then
   planner, then generators. Each milestone ends with a code review pass and
   the demo query recorded as a replay fixture so it runs in CI for free.

Every agent works on its own branch off `claude/callgebra-sql-rlm-9opg46`,
with `cargo fmt`, `cargo clippy -D warnings` and `cargo test` green before
merge. The coordinating session owns the workspace `Cargo.toml`, the
interface crate and the merge order.
