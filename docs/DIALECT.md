# CallSQL dialect reference

CallSQL is the PostgreSQL-flavoured subset Kleene executes, plus the
statements and functions that make model calls, tools, delegation, budgets
and planning first-class. Everything the model does in a session is one or
more CallSQL statements inside a single ```sql fence.

## Relational core

Statements: `SELECT` (with `WITH`, `WITH RECURSIVE`, `JOIN ... ON`, `CROSS
JOIN`, `CROSS JOIN LATERAL`, `WHERE`, `GROUP BY` / `HAVING`, `ORDER BY`,
`LIMIT` / `OFFSET`, `DISTINCT`, `UNION [ALL]`, `EXISTS` / `NOT EXISTS`, `IN`,
scalar subqueries, `CASE`, `CAST`), `VALUES`, `CREATE TABLE t AS SELECT ...`,
`CREATE TABLE t (...)`, `INSERT INTO t SELECT ...`, `DROP TABLE [IF EXISTS] t`.

Types: `BOOLEAN`, `BIGINT`, `DOUBLE`, `TEXT`, `JSON`, `VECTOR`. Three-valued
logic with `AND` / `OR` short-circuiting left to right, which the planner
relies on (cheap conjuncts first).

String literals are single-quoted with `''` for a quote, or dollar-quoted:
`$$...$$` takes any text verbatim, quotes and newlines included, which is how
a whole program goes into `CALL write_file(path, $$...$$)`.

Recursive CTEs run by semi-naive evaluation: the recursive term sees only the
previous round's delta. `UNION` (set) terminates when a round adds nothing new;
`UNION ALL` (bag) runs until the term yields no rows or the round cap
(`SET max_recursion_rounds = n`). A trailing `ORDER BY ... LIMIT k` inside a
recursive CTE keeps the best `k` new rows of each round: a beam of width `k`.

Builtins include the usual string, numeric, null and JSON functions and
`generate_series(a, b)`. The relational core is tested differentially against
DuckDB.

## Sessions

Every run starts with `ctx(ordinal, text)` when a context is supplied, one
row per paragraph (blank-line separated). Tables created in a session persist
in the store; a child session gets its own namespace.

`FINAL FROM (query)` ends the session with that relation as the answer.

`SET key = value`: `effort`, `model.default`, `max_tokens`, `memo` (on/off),
`cache_prefix`, `max_recursion_rounds`, `budget.calls`, `budget.tokens`,
`budget.dollars`, `budget.depth`.

`EXPLAIN statement` prints the call plan: per operator the estimated rows,
calls, tokens and dollars, the call kinds (`λ` scalar, `κ` table, `ρ`
recursive, `tool`), volatility fences, the complexity fragment (CQ / FO /
REC), the rules that fired, the join-order plan space and the alternatives
priced. `EXPLAIN ANALYZE` also runs it and prints actuals. A statement whose
estimate exceeds the remaining budget is refused with its plan.

## Model calls

Scalar builtins: `llm(prompt [, alias [, effort]]) -> TEXT`,
`llm_bool(prompt) -> BOOLEAN`, `llm_json(prompt, schema) -> JSON`. Table
builtins: `expand(text, n) -> TABLE(item TEXT)` (used as `CROSS JOIN LATERAL
expand(h, 3) AS e`).

Prompt-defined functions:

```sql
CREATE [OR REPLACE] FUNCTION name(arg TYPE, ...) RETURNS TYPE
  AS PROMPT 'template with {arg} placeholders'
  [MODEL 'alias'] [IMMUTABLE | STABLE | VOLATILE]
  [PROXY score_fn THRESHOLDS (low, high)];
CREATE FUNCTION name(...) RETURNS TYPE AS SQL (SELECT ...);
CREATE FUNCTION name(...) RETURNS BOOLEAN AS SHELL 'command {arg}';
```

`MODEL` picks the tier (`root`, `worker`, `proxy`, `judge`, or any alias the
router knows). `PROXY` declares a cheap scorer (`RETURNS DOUBLE` in `[0, 1]`)
the planner may cascade the predicate through. Volatility defaults: prompt
functions `IMMUTABLE`, SQL bodies `STABLE`, shell bodies `VOLATILE`.

Identical prompts are served from the memo (`memo(model, fingerprint,
response, input_tokens, output_tokens, cost_usd, created_at)`, keyed by
model and prompt fingerprint) across sessions and runs.

## Tools

Pure table functions usable in `FROM`: `files(glob)`, `lines(path)`,
`read(path)`, `grep(pattern [, glob])`, `chunks(text, size [, overlap])`,
`env(name)`, `git_log([n])`, `git_diff([ref])`, `git_blame(path)`. Volatile
tools run as statements: `CALL shell('cmd' [, cwd, timeout_ms]) [FROM query]`,
`CALL write_file(path, text)`, `CALL append_file(path, text)`,
`CALL patch(path, old, new)` (one exact occurrence, after the file was read
in this session), `CALL mkdir(path)`, `CALL remove(path)`,
`CALL web_fetch(url)`, `CALL web_search(q [, n])`. A tool's output relation is
rendered back to the model; the full table with columns and volatility is
in `kleene-tools`'s crate docs. Roles restrict which tools a session may use.

## Delegation

- `rlm(question [, context])` in `CROSS JOIN LATERAL`: one child session per
  input row at depth + 1, returning `(answer, detail JSON)`; `spawn` also
  returns the child's `session` id.
- `CREATE AGENT name MODEL 'alias' EFFORT 'low' TOOLS (files, grep)
  BUDGET (calls 40) PROMPT '...'` declares a role; `spawn('name', task [,
  context])` runs it. The child's catalog is restricted to the role's tools;
  its budget is the parent's remaining slice intersected with the role budget.

Depth is bounded by the session's `budget.depth`; children's spending rolls
up into the parent.

## Planner rules

1. Cheap first: pure conjuncts before call conjuncts, in filters and join
   conditions.
2. Dedupe then map: identical prompts cost one call (memo).
3. Batch: not yet.
4. Cascade: `σ oracle(x)` with a declared proxy becomes `score >= high OR
   (score >= low AND oracle(x))`, applied when the priced cascade is cheaper.
5. Semi-join: `[NOT] EXISTS` with a call predicate becomes a semi/anti join
   that stops at the first (counter)example.
6. Beam-limited recursion: `ORDER BY ... LIMIT k` in a recursive CTE.
7. Join ordering: dynamic programming over relation subsets with
   branch-and-bound, call predicates priced per surviving pair.
8. Volatility fences: nothing moves across a `VOLATILE` operator.

Estimates use the store's row counts, sampled selectivity (observed pass rates
of boolean call predicates), per-alias cost factors and round-by-round
recursion costing.

## The continual loop

`kleene learn` keeps a `tasks` table fed by generators with code oracles,
runs the curriculum's pick through a session, judges `FINAL`, moves
Bradley-Terry ratings, and adopts winning SQL into a `playbook` only after a
replay eval; see `demos/README.md`. Learned entries appear in the prompt as a
marked section. `kleene bench` runs task packs (`tasks/*/pack.json`) under
`learning`, `frozen` and `plain` (tool-calling agent) modes and records every
task in `evals`.
