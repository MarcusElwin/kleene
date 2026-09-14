# Demos

Both demos run the M3 harness end to end: the model writes CallSQL, the
harness executes it, and children run for `rlm(...)` and `spawn(...)`.
They need a provider: set `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` (or a
`KLEENE_ROUTER_TOML`). Every model call is memoised in the store, so a
second run of the same demo costs nothing.

## Long context: partition and map (`demos/oolong`)

Sixty dated meeting notes about six projects. The question needs every note,
not one lookup, so the root peeks at `ctx`, partitions it and maps `rlm` over
the partitions, then aggregates the children's answers.

```bash
kleene run @demos/oolong/task.txt --context demos/oolong/corpus.txt --budget-calls 60
kleene trace "SELECT depth, role, outcome, turns, calls FROM trace_sessions ORDER BY started_at"
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
kleene run @demos/repo-review/task.txt --workspace . --max-depth 1
kleene trace "SELECT s.role, st.sql, st.calls FROM trace_statements st JOIN trace_sessions s USING (session) ORDER BY st.started_at"
```

## Watching live in the TUI

`kleene tui` attaches to the engine daemon (starting one in the
background if none is listening) and shows the call tree, transcript and plan
of every run as events arrive. `--run` starts a task on connect:

```bash
kleene tui --run @demos/oolong/task.txt --context demos/oolong/corpus.txt
```

Keys: `j`/`k` move, `f` fold, `x` cancel the selected statement, `3` opens
the trace explorer (SQL over the store), `d` detaches while the run continues.
`kleene attach` is the headless twin: it prints every event as a JSON
line, and with `--run` exits when that run finishes. `kleene daemon` runs
the engine in the foreground; a client that reconnects resumes from its last
cursor.

## Planner (`demos/planner`)

`EXPLAIN` scripts showing join ordering over call predicates, cascades and
beam-limited recursion: see [`planner/README.md`](planner/README.md).

## Continual loop

`kleene learn` keeps a `tasks` table fed by generators (`sat3`, `graph`,
`puzzle`, `corpus`, `repo`), each with a code oracle, plus tasks you add and
tasks the model proposes (judged by a separate `judge` call). Every attempt
moves a Bradley-Terry rating for the task and for the solver configuration;
the curriculum picks the pending task nearest even odds and steps a
generator's dial up past a 70% solve rate and down below 30%. SQL that
solved a task becomes a playbook candidate, adopted only if it solves at
least as many replayed tasks of that kind at no more cost, and shown to
later sessions of the same kind as a learned example. All of it is tables
in the store, so stopping and starting again resumes.

```bash
kleene learn generate puzzle --count 5
kleene learn add "How many .rs files are under crates/kleene-sql?" --kind repo --expect "9"
kleene learn run --tasks 20 --budget-dollars 2 --generators puzzle,corpus,graph
kleene learn board          # pending / running / solved / failed / review per generator, with dials
kleene learn report         # SELECT generator, difficulty, AVG(solved), AVG(calls), AVG(depth) FROM trace_tasks GROUP BY 1, 2
kleene learn playbook       # the version ledger with eval notes
kleene learn revert 3       # withdraw a playbook version
kleene tui                  # view 4 is the live board
```

## Benchmarks

`kleene bench` runs a task pack (`tasks/<pack>/pack.json`) in one of three
modes and records every task in `evals`: `learning` (playbook shown and
adopted), `frozen` (the control: no playbook), `plain` (a tool-calling agent
on the same provider, tools and budgets, the cost-parity baseline).

```bash
kleene bench run tasks/oolong-like --mode frozen --record fixtures/oolong   # record model replies
kleene bench run tasks/oolong-like --mode learning --replay fixtures/oolong # replay offline
kleene bench run tasks/terminal --mode plain
kleene bench report          # accuracy, calls and dollars per pack and mode
kleene bench curve <run-id>  # the learning curve as a sparkline and rolling mean
kleene bench csv > evals.csv # every eval row, for plotting
kleene bench import-lab ~/harvey-labs tasks/harvey-lab
```

## Resuming

A run that hits its turn cap (or is interrupted) can be continued:

```bash
kleene run "..." --max-turns 5
kleene resume <run-id> --max-turns 10
```

The transcript, defined functions, agents and budget are persisted in the
store after every turn (`kleene_sessions`).
