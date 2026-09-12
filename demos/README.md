# Demos

Both demos run the M3 harness end to end: the model writes CallSQL, the
harness executes it, and children run for `rlm(...)` and `spawn(...)`.
They need a provider: set `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` (or a
`CALLGEBRA_ROUTER_TOML`). Every model call is memoised in the store, so a
second run of the same demo costs nothing.

## Long context: partition and map (`demos/oolong`)

Sixty dated meeting notes about six projects. The question needs every note,
not one lookup, so the root peeks at `ctx`, partitions it and maps `rlm` over
the partitions, then aggregates the children's answers.

```bash
callgebra run @demos/oolong/task.txt --context demos/oolong/corpus.txt --budget-calls 60
callgebra trace "SELECT depth, role, outcome, turns, calls FROM trace_sessions ORDER BY started_at"
```

`demos/oolong/answer.txt` holds the ground truth (computed when the corpus
was generated). A plain SQL solution exists too, since the notes are regular:
the interesting part is watching which strategy the model picks and what it
costs; `EXPLAIN` shows the estimate before the spend.

## Repository question with reviewer agents (`demos/repo-review`)

The root reads the codebase with `files`, `grep` and `read`, proposes
hypotheses, declares a `reviewer` agent restricted to read-only tools, and
spawns one reviewer per hypothesis with `CROSS JOIN LATERAL spawn(...)`.

```bash
callgebra run @demos/repo-review/task.txt --workspace . --max-depth 1
callgebra trace "SELECT s.role, st.sql, st.calls FROM trace_statements st JOIN trace_sessions s USING (session) ORDER BY st.started_at"
```

## Watching live in the TUI

`callgebra tui` attaches to the engine daemon (starting one in the
background if none is listening) and shows the call tree, transcript and plan
of every run as events arrive. `--run` starts a task on connect:

```bash
callgebra tui --run @demos/oolong/task.txt --context demos/oolong/corpus.txt
```

Keys: `j`/`k` move, `f` fold, `x` cancel the selected statement, `3` opens
the trace explorer (SQL over the store), `d` detaches while the run continues.
`callgebra attach` is the headless twin: it prints every event as a JSON
line, and with `--run` exits when that run finishes. `callgebra daemon` runs
the engine in the foreground; a client that reconnects resumes from its last
cursor.

## Resuming

A run that hits its turn cap (or is interrupted) can be continued:

```bash
callgebra run "..." --max-turns 5
callgebra resume <run-id> --max-turns 10
```

The transcript, defined functions, agents and budget are persisted in the
store after every turn (`callgebra_sessions`).
