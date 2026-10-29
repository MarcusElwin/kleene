# CallSQL dialect reference

CallSQL is the PostgreSQL-flavoured subset Kleene executes, plus the
statements and functions that make model calls, tools, delegation, budgets
and planning first-class. Everything the model does in a session is one or
more CallSQL statements inside a single ```sql fence. `kleene dialect`
prints the copy of this page compiled into the binary, so it always
describes the build you have.

## Relational core

Statements: `SELECT` (with `WITH`, `WITH RECURSIVE`, `JOIN ... ON`, `CROSS
JOIN`, `CROSS JOIN LATERAL`, `WHERE`, `GROUP BY` / `HAVING`, `ORDER BY`,
`LIMIT` / `OFFSET`, `DISTINCT`, `UNION [ALL]`, `EXISTS` / `NOT EXISTS`, `IN`,
scalar subqueries, `CASE`, `CAST`), `VALUES`, `CREATE TABLE t AS SELECT ...`,
`CREATE TABLE t (col TYPE [NOT NULL], ...)` (an empty table; other column
constraints are accepted and ignored), `INSERT INTO t VALUES (...), (...)`,
`INSERT INTO t SELECT ...`, `DROP TABLE [IF EXISTS] t`. Several statements
go in one reply separated by `;`, `CREATE FUNCTION` and `FINAL` included;
execution stops at the first error.

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

Builtins: `upper`, `lower`, `trim`, `ltrim`, `rtrim`, `length`, `substr`,
`left`, `right`, `strpos`, `split_part`, `replace`, `concat` (and `||`),
`repeat`, `lpad`, `rpad`, `reverse`, `chr`, `ascii`, `starts_with`,
`contains`, `LIKE` / `ILIKE`, `coalesce`, `nullif`, `greatest`, `least`,
`abs`, `round`, `floor`, `ceil`, `mod`, `json_extract`,
`json_extract_string`, the aggregates `COUNT`, `SUM`, `AVG`, `MIN`, `MAX`,
`STRING_AGG`, and the table function `generate_series(a, b)`. Lengths and
positions count characters, not bytes, so Japanese text measures as
written. Anything else is `unknown function`; DuckDB's `read_csv_auto`
and friends are not part of CallSQL (load files with `--context`, or the
`read` and `lines` tools). The relational core is tested differentially
against DuckDB.

## Sessions

Every run starts with `ctx(ordinal, text)` when a context is supplied
(`kleene run --context file`), one row per paragraph: paragraphs are
separated by a blank line, and a file with no blank line is a single row,
so put one record per paragraph when each should be its own row. Tables
created in a session persist in the store; a child session gets its own
namespace.

`FINAL` ends the session with an answer relation, in three forms:
`FINAL(expr)` (one row, column `answer`), `FINAL SELECT ...` (also `FINAL
WITH ... SELECT ...` and `FINAL VALUES (...)`), and `FINAL FROM (query)`.
`FINAL` is one statement: `FINAL SELECT 1; SELECT 2` is an error.

`rlm`, `spawn` and the other delegation functions need a running harness
(`kleene run`, the TUI): `kleene repl` executes CallSQL against the store
without one and reports `rlm needs a running harness` instead.

`SET key = value`: `effort`, `model.default`, `max_tokens`, `memo` (on/off),
`cache_prefix`, `max_recursion_rounds`, `budget.calls`, `budget.tokens`,
`budget.dollars`, `budget.depth`.

`EXPLAIN statement` prints the call plan: per operator the estimated rows,
calls, tokens and dollars, the call kinds (`λ` scalar, `κ` table, `ρ`
recursive, `tool`), volatility fences, the complexity fragment (CQ / FO /
REC), the rules that fired, the join-order plan space and the alternatives
priced. `EXPLAIN ANALYZE` also runs it and prints actuals. A statement whose
estimate exceeds the remaining budget is refused with its plan.

The estimate prices the statement's own calls: a scalar call such as
`llm_bool(...)` is assumed to be about 600 tokens in and 150 out, and
`rlm`/`spawn` a whole child session (see [Delegation](#delegation)). It does
not include the turn that carries the statement itself, whose system
prompt, catalog and transcript cost a few thousand tokens per turn: that
is the session's overhead, shown in the budget line after each turn. Call
counts are exact for scalar calls; tokens and dollars are a forecast, and
`EXPLAIN ANALYZE` is the way to calibrate it: it prints the estimate and
the actual side by side, and the measured selectivities and rows per call
feed the next estimate. Dollars are only forecast and enforced for models
with a price: the provider's own cost report, the Anthropic and OpenAI rate
cards, a `[pricing]` section in the router TOML, or `KLEENE_PRICING`.

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
  [PROXY score_fn [THRESHOLDS (low, high)]];
CREATE FUNCTION name(...) RETURNS TYPE AS SQL (SELECT ...);
CREATE FUNCTION name(...) RETURNS BOOLEAN AS SHELL 'command {arg}';
CALIBRATE name [SAMPLE n] [RECALL r] [PRECISION p] FROM query;
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

`CALIBRATE` sets a predicate's thresholds from a sample instead of a guess:
the query's columns are the predicate's arguments, its first `n` rows (20
by default) are scored by the proxy and the oracle, and the thresholds are
the lowest `high` at which the rows the proxy accepts outright are right in
`p` of cases and the highest `low` that still keeps `r` of the oracle's
positives (0.9 each by default). The result names the thresholds and what
the sample would have cost: how many rows the proxy decides alone, how many
ask the oracle, how many are rejected. The thresholds are remembered per
template and proxy in `calibrations`, so a later `CREATE FUNCTION` with
`PROXY score_fn` and no `THRESHOLDS` starts from them; declared thresholds
always win, and the defaults are `(0.2, 0.8)`.

Identical prompts are served from the memo (`memo(model, fingerprint,
response, input_tokens, output_tokens, cost_usd, created_at)`, keyed by
model and prompt fingerprint) across sessions and runs.

## Tools

Pure table functions usable in `FROM`: `files(glob)`, `lines(path [, from,
to])`, `read(path)`, `read_csv(path)`, `read_json(path)`, `grep(pattern [,
glob])`, `search(query [, glob, n])` (files ranked by BM25 against the
query's words, with the best line of each), `chunks(text, size [,
overlap])`, `env(name)`, `git_log([n])`, `git_diff([ref])`,
`git_blame(path)`, `skill(name)` (the body of a loaded skill).

`read_csv` and `read_json` return `(row, record JSON)`, one record per CSV
line (keyed by the header) or JSON element (an array, an object holding one
array, or JSON Lines); pick columns with `json_extract_string`:

```sql
CREATE TABLE threads AS
SELECT json_extract_string(record, '$.id') AS id,
       json_extract_string(record, '$.text') AS text
FROM read_csv('threads.csv');
```

`kleene run --load threads=threads.csv` (also `kleene repl --load`) does
the same before the first turn with typed columns (`BIGINT`, `DOUBLE`,
`BOOLEAN`, `JSON` or `TEXT`, inferred per column), so the model starts with
the table in its catalog. Files are UTF-8; a byte-order mark is fine. Volatile tools run as statements: `CALL shell('cmd' [, cwd,
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

Four tables have a meaning to the harness. `turns(run, n, sql, result)`
holds every turn of every run: once more than sixteen turns have run, the
results of all but the last eight are replaced in the transcript by a
one-line stub naming the row, and live only there. `plan(step, status)`, if
the model creates it, is the task's plan: the latest row per step counts,
statuses are `todo`, `doing` and `done`, and the UI renders it under the
session. `conversations(conversation, run, n, role, text, ts)` holds the
task and the answer of every run started in a conversation (the TUI's
prompt; `/new` starts another), and a follow-up run is told to read it.
`memory(key, value)` persists across runs and conversations for whatever
the model chooses to `INSERT INTO` it.

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

- `spawn_async('name', task [, context])` starts the agent and returns one
  row `(handle, session)` at once; the child runs under the parent's
  concurrency and budget slice while the parent carries on. `await(handle)`
  in `FROM` (or `CROSS JOIN LATERAL`) waits for that child and returns its
  `(answer, detail, session)` rows; a child nobody awaited is joined when the
  parent ends, so its spending is always charged. `inbox()` is a table of
  `(from_session, ts, text)`: the messages children posted with
  `CALL send('parent', text)`; `CALL send(handle, text)` messages a child
  (one delivery row). A handle belongs to the session that spawned it.

Depth is bounded by the session's `budget.depth`; children's spending rolls
up into the parent.

A child session is a whole agent loop, not one model call: every turn
carries the agent system prompt and the child's context, so one `rlm` row
typically costs several calls and ten thousand tokens or more. `EXPLAIN`
prices `rlm`/`spawn` that way (a default of four calls and twelve thousand
tokens per child, replaced by the store's own average once three children
have finished; the `learned:` line says which) rather than as one call.
Delegate once and keep the rows, then answer from the table:

```sql
CREATE TABLE results AS
SELECT split_part(c.text, ' | ', 1) AS id, r.answer
FROM ctx c
CROSS JOIN LATERAL rlm('Summarise this thread in one line: ' || c.text) r;

FINAL FROM (SELECT id, answer FROM results ORDER BY id);
```

A `FINAL` whose query calls `rlm` or `spawn` is refused with that pattern,
because a `FINAL` that fails or runs out of budget would repeat every child.

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

Estimates use the store's row counts, learned and sampled selectivity, learned
branching, per-alias cost factors and round-by-round recursion costing. Every
statement records what it observed in the store's `estimates` table: the pass
rate of each prompt-defined predicate (keyed by the hash of its template, so a
refined prompt starts afresh) and the rows per call of each table function
(`expand`, tools, `rlm`), weighted into earlier sessions' counts. The next
session's planner reads them back for the functions it can see, and this
session's own observations override them once there are a few. `EXPLAIN`
lists the ones a plan relies on: `learned: relevant: selectivity 0.25 from 40
rows; expand: 2.0 rows per call from 10 calls`.

## The continual loop

`kleene learn` keeps a `tasks` table fed by generators with code oracles,
runs the curriculum's pick through a session, judges `FINAL`, moves
Bradley-Terry ratings, and adopts winning SQL into a `playbook` only after a
replay eval; see `demos/README.md`. Learned entries appear in the prompt as a
marked section. `kleene bench` runs task packs (`tasks/*/pack.json`) under
`learning`, `frozen` and `plain` (tool-calling agent) modes and records every
task in `evals`.
