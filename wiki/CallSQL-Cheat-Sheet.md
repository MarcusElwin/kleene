# CallSQL cheat sheet

CallSQL is the PostgreSQL-flavoured subset Kleene executes, plus the statements and functions that make model calls, tools, delegation, budgets and planning first-class. Everything the model does in a session is CallSQL inside a single ```sql fence. The full reference is [`docs/DIALECT.md`](https://github.com/MarcusElwin/kleene/blob/main/docs/DIALECT.md).

## Relational core

`SELECT` with `WITH` and `WITH RECURSIVE`, `JOIN ... ON`, `CROSS JOIN [LATERAL]`, `WHERE`, `GROUP BY` / `HAVING`, `ORDER BY`, `LIMIT` / `OFFSET`, `DISTINCT`, `UNION [ALL]`, `[NOT] EXISTS`, `IN`, scalar subqueries, `CASE`, `CAST`; `VALUES`; `CREATE TABLE t AS SELECT ...`; `CREATE TABLE t (...)`; `INSERT INTO t SELECT ...`; `DROP TABLE [IF EXISTS] t`.

Types: `BOOLEAN`, `BIGINT`, `DOUBLE`, `TEXT`, `JSON`, `VECTOR`. Three-valued logic; `AND` / `OR` short-circuit left to right, which the planner relies on. Strings are single-quoted, or dollar-quoted (`$$...$$`) for verbatim text such as a whole file.

Recursive CTEs run by semi-naive evaluation. `UNION` stops when a round adds nothing new; `UNION ALL` runs until the term yields no rows or `SET max_recursion_rounds = n`. A trailing `ORDER BY ... LIMIT k` inside the recursive term keeps the best `k` new rows per round: a beam of width `k`.

## Sessions

| Statement | Does |
|---|---|
| `ctx(ordinal, text)` | the context table every run starts with, one row per paragraph |
| `FINAL FROM (query)` | ends the session with that relation as the answer |
| `SET key = value` | `effort`, `model.default`, `max_tokens`, `memo`, `cache_prefix`, `max_recursion_rounds`, `budget.calls`, `budget.tokens`, `budget.dollars`, `budget.depth` |
| `EXPLAIN stmt` | the call plan: rows, calls, tokens, dollars per operator, call kinds, fences, rules fired, join-order plan space |
| `EXPLAIN ANALYZE stmt` | also runs it and prints actuals |

A statement whose estimate exceeds the remaining budget is refused with its plan.

## Model calls

```sql
llm(prompt [, alias [, effort]])   -> TEXT
llm_bool(prompt)                   -> BOOLEAN
llm_json(prompt, schema)           -> JSON        -- schema is JSON Schema, or a bare field map like {"hours": "number"}
expand(text, n)                    -> TABLE(item TEXT)   -- CROSS JOIN LATERAL expand(h, 3) AS e
```

Prompt-defined functions:

```sql
CREATE [OR REPLACE] FUNCTION name(arg TYPE, ...) RETURNS TYPE
  AS PROMPT 'template with {arg} placeholders'
  [MODEL 'alias'] [BATCH n] [IMMUTABLE | STABLE | VOLATILE]
  [PROXY score_fn THRESHOLDS (low, high)];
CREATE FUNCTION name(...) RETURNS TYPE AS SQL (SELECT ...);
CREATE FUNCTION name(...) RETURNS BOOLEAN AS SHELL 'command {arg}';
```

- `MODEL` picks the alias (`root`, `worker`, `proxy`, `judge`, or any the router knows).
- `BATCH n` answers up to `n` distinct argument tuples per call; `EXPLAIN` prices `ceil(rows / n)` calls.
- `PROXY` declares a cheap scorer in `[0, 1]` the planner may cascade the predicate through.
- Volatility defaults: prompt functions `IMMUTABLE`, SQL bodies `STABLE`, shell bodies `VOLATILE`.

Identical prompts are served from the memo across sessions and runs.

## Tools

Pure table functions, usable in `FROM`: `files(glob)`, `lines(path [, from, to])`, `read(path)`, `grep(pattern [, glob])`, `search(query [, glob, n])`, `chunks(text, size [, overlap])`, `env(name)`, `git_log([n])`, `git_diff([ref])`, `git_blame(path)`, `skill(name)`.

Volatile tools run as statements:

```sql
CALL shell('cmd' [, cwd, timeout_ms]) [FROM query];
CALL write_file(path, text);   CALL append_file(path, text);
CALL patch(path, old, new);    -- one occurrence, fuzzy on whitespace and quotes; returns the diff
CALL mkdir(path);              CALL remove(path);
CALL web_fetch(url);           CALL web_search(q [, n]);
```

Tools of a connected MCP server appear as `<server>_<tool>`: read-only ones as table functions, the rest through `CALL`. Roles restrict which tools a session may use.

Two tables mean something to the harness: `turns(n, sql, result)` holds earlier turns (old results fold out of the prompt after sixteen turns), and `plan(step, status)`, if the model creates it, is the task's plan with statuses `todo`, `doing`, `done`, rendered by the UI.

## Delegation

```sql
SELECT p.chunk, r.answer
FROM parts p CROSS JOIN LATERAL rlm('How many hours per project?', p.chunk) r;   -- one child per row, (answer, detail)

CREATE AGENT reviewer MODEL 'worker' EFFORT 'low' TOOLS (files, grep, read)
  BUDGET (calls 40) PROMPT 'You review one hypothesis and report evidence.';
SELECT h.text, s.answer
FROM hypotheses h CROSS JOIN LATERAL spawn('reviewer', h.text) s;              -- (answer, detail, session)
```

A child is the same loop one level deeper, with the role's tools, the parent's remaining budget slice intersected with the role budget, and its own table namespace. Depth is bounded by `budget.depth`; children's spending rolls up.

## Planner rules

1. Cheap first: pure conjuncts before call conjuncts.
2. Dedupe then map: identical prompts cost one call.
3. Cascade: an oracle with a declared proxy becomes `score >= high OR (score >= low AND oracle(x))` when that is cheaper.
4. Semi-join: `[NOT] EXISTS` with a call predicate stops at the first (counter)example.
5. Beam-limited recursion: `ORDER BY ... LIMIT k` in a recursive CTE.
6. Join ordering: dynamic programming over relation subsets, call predicates priced per surviving pair.
7. Volatility fences: nothing moves across a `VOLATILE` operator.
