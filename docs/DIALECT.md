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
`llm_bool(prompt) -> BOOLEAN`, `llm_json(prompt, schema) -> JSON`. The
schema is JSON Schema, and the Anthropic adapter rewrites it into the
subset the structured-output API accepts before sending: a bare field map
such as `{"hours": "number"}` becomes a closed object with every field
required, an object gets `additionalProperties: false`, and an array's
`minItems` is clamped to 1 (a schema the API would otherwise reject fails
the call, and a per-row `llm_json` then fails on every row). Table
builtins: `expand(text, n) -> TABLE(item TEXT)` (used as `CROSS JOIN LATERAL
expand(h, 3) AS e`).

Prompt-defined functions:

```sql
CREATE [OR REPLACE] FUNCTION name(arg TYPE, ...) RETURNS TYPE
  AS PROMPT 'template with {arg} placeholders'
  [MODEL 'alias'] [BATCH n] [IMMUTABLE | STABLE | VOLATILE]
  [PROXY score_fn THRESHOLDS (low, high)];
CREATE FUNCTION name(...) RETURNS TYPE AS SQL (SELECT ...);
CREATE FUNCTION name(...) RETURNS BOOLEAN AS SHELL 'command {arg}';
```

`MODEL` picks the tier (`root`, `worker`, `proxy`, `judge`, or any alias the
router knows). A prompt function is batched: the executor answers up to `n`
distinct argument tuples per call (the template shown once, the items
numbered, one `{"item": i, "answer": ...}` back per item) and `EXPLAIN`
prices `ceil(rows / n)` calls. `n` is 20 unless `BATCH n` says otherwise;
`BATCH 1` turns batching off for long or dependent items. Items an answer
leaves out or mistypes are asked again in one more batched call, then one
call each, so a batch changes cost, never a result. `PROXY` declares a cheap scorer (`RETURNS DOUBLE` in `[0, 1]`)
the planner may cascade the predicate through. Volatility defaults: prompt
functions `IMMUTABLE`, SQL bodies `STABLE`, shell bodies `VOLATILE`.

Identical prompts are served from the memo (`memo(model, fingerprint,
response, input_tokens, output_tokens, cost_usd, created_at)`, keyed by
model and prompt fingerprint) across sessions and runs.

## Tools

Pure table functions usable in `FROM`: `files(glob)`, `lines(path [, from,
to])`, `read(path)`, `grep(pattern [, glob])`, `search(query [, glob, n])`
(files ranked by BM25 against the query's words, with the best line of
each), `chunks(text, size [, overlap])`, `env(name)`, `git_log([n])`,
`git_diff([ref])`, `git_blame(path)`, `skill(name)` (the body of a loaded
skill). Volatile tools run as statements: `CALL shell('cmd' [, cwd,
timeout_ms]) [FROM query]`, `CALL write_file(path, text)`,
`CALL append_file(path, text)`, `CALL patch(path, old, new)` (one occurrence,
exact or else ignoring whitespace and typographic quotes, after the file was
read in this session; returns the unified diff), `CALL mkdir(path)`,
`CALL remove(path)`, `CALL web_fetch(url)`, `CALL web_search(q [, n])`. A
tool's output relation is rendered back to the model; the full table with
columns and volatility is in `kleene-tools`'s crate docs. Roles restrict
which tools a session may use.

Tools of a connected MCP server (`kleene mcp`) appear as `<server>_<tool>`
with the same split: a tool the server marks read-only is a table function,
any other is `VOLATILE` and runs through `CALL`. Arguments are positional,
required ones first in the server's order, then the optional ones by name;
each result row is one text item the server returned.

Two tables have a meaning to the harness. `turns(n, sql, result)` holds
every earlier turn of the session: once more than sixteen turns have run,
the results of all but the last eight are replaced in the transcript by a
one-line stub and live only there. `plan(step, status)`, if the model
creates it, is the task's plan: the latest row per step counts, statuses
are `todo`, `doing` and `done`, and the UI renders it under the session.

When a run has a finish check (`kleene run --check`, a pack task's `check`),
a `FINAL` runs the command first; a non-zero exit refuses the `FINAL` with
the output's tail, and the session continues.

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
3. Batch: a prompt function over n distinct tuples costs `ceil(n / batch)`
   calls, batch 20 by default.
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
