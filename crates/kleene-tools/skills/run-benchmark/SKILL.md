---
name: run-benchmark
description: Run a task pack with kleene bench in learning, frozen or plain mode, record or replay fixtures, and read the report and plots
---

# Run a benchmark

`kleene bench` runs a pack of tasks under one of three modes and writes a
row per task to `evals` in the store. Everything here is a shell command;
from a session use `CALL shell('kleene bench ...')`.

## Packs

The shipped packs are under `tasks/`: `terminal`, `oolong-like`,
`finance-synthetic`, `legal-synthetic`, `logbook-hard`, `memo-rubric`,
`coding`. `tasks/<pack>/pack.json` lists the tasks; `kleene bench build`
makes new ones from a generator, `kleene bench import-oolong` and
`import-lab` fetch public benchmarks.

## Modes

| mode | what runs | compares |
|---|---|---|
| `learning` | Kleene with the playbook on; solved tasks become playbook candidates | the treatment |
| `frozen` | Kleene with the playbook off | the control for learning |
| `plain` | a native tool-calling agent on the same provider, tools and budget | the control for the SQL abstraction |

## Run

```bash
kleene bench run tasks/coding --mode frozen --limit 4
kleene bench run tasks/coding --mode plain
kleene bench run tasks/memo-rubric --mode learning --record fixtures/memo   # keep every reply
kleene bench run tasks/memo-rubric --mode learning --replay fixtures/memo   # offline rerun
kleene bench run tasks/coding --mode frozen --resume <run-id>              # continue after an abort
```

Use a fresh store per mode (`--db bench-frozen.duckdb`) when the cost
columns matter: two modes in one store share the memo and the second one
runs cheaper than it should.

## Read

```bash
kleene bench report          # accuracy, calls, tokens, dollars per pack and mode
kleene bench csv > evals.csv # one row per task
kleene bench plot plots/     # results, cost parity, learning curve, calls vs difficulty, estimate accuracy
kleene trace "SELECT task, solved, detail FROM evals WHERE run = '<run-id>' ORDER BY seq"
```

A coding task counts as solved when the grader's hidden tests pass. A task
whose `detail` says `no FINAL` ran out of turns or budget; raise
`--max-turns` or `--budget-calls` before blaming the model.
