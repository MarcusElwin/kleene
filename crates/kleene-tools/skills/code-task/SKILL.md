---
name: code-task
description: Implement or fix code in the workspace with the edit, run, read loop in CallSQL, keeping a plan table and finishing only when the tests pass
---

# A coding task

Code tasks are a loop: find the file, read it, change it, run the tests,
read the failure, change it again. In CallSQL each step is one statement.

## Find and read

```sql
SELECT path, lineno, snippet FROM search('parse transactions amount', '*.py', 5);
SELECT path, lineno, text FROM grep('def parse_', '*.py');
SELECT lineno, text FROM lines('ledger/parse.py', 1, 60);
```

`search` ranks files by the words of a query; `grep` matches a regex;
`lines` with a range keeps the result small. Read a file before patching
it: `patch` refuses a file the session has not read.

## Change

```sql
CALL write_file('ledger/parse.py', $$...whole file...$$);
CALL patch('ledger/parse.py', $$    return rows$$, $$    return [normalise(r) for r in rows]$$);
```

`patch` replaces one occurrence (exact, else ignoring whitespace and
quotes) and returns the diff. Several edits to one file go in one
statement: `CALL patch('f.py', old, new) FROM (VALUES (...), (...)) AS edits(old, new)`.
Dollar quotes `$$ ... $$` need no escaping inside.

## Run and read

```sql
CALL shell('python3 -m unittest -q');
```

Read `stderr` and `exit_code`. Fix the first failure, run again. Never
edit the tests to make them pass.

## Plan

For a task with several steps, keep a plan the UI shows:

```sql
CREATE TABLE plan AS SELECT * FROM (VALUES ('read the tests', 'doing'), ('implement parse', 'todo'), ('run tests', 'todo')) AS p(step, status);
INSERT INTO plan VALUES ('read the tests', 'done'), ('implement parse', 'doing');
```

The latest row per step counts. Statuses are `todo`, `doing`, `done`.

## Finish

`FINAL` is accepted only when the run's check command (the task's tests)
exits 0; a refused `FINAL` comes back with the failing output. Finish with
one row: `FINAL FROM (SELECT true AS done, 'what changed' AS note);`
