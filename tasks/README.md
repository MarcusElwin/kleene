# Task packs

A pack is a directory with a `pack.json`: a name, a description, a license
note, a default task kind and a list of tasks, each with its text, optional
context (inline or a file), optional setup commands and source directory for
a fresh workspace, an oracle (`exact`, `number`, `sat`, `shell`, `judge`,
`oolong`, `human`) and a difficulty prior. `kleene bench run <dir> --mode
learning|frozen|plain` runs one; `kleene bench report` summarises every run.
What the modes, oracles, report columns and plots mean is in
[`docs/BENCHMARKS.md`](../docs/BENCHMARKS.md).

| Pack | What it is | Oracle |
|---|---|---|
| `terminal/` | six Terminal-Bench-style shell tasks in a fresh workspace (create a file, count lines, rename extensions, grep, sum a CSV column, write JSON) | shell commands over the files left behind |
| `oolong-like/` | 20 meeting-note corpora with per-project hour totals, frozen from the `corpus` generator at dial 0.5 | exact rows |
| `finance-synthetic/` | 20 synthetic statements with growth, ratio and sum questions, frozen from `statements` | number within tolerance |
| `legal-synthetic/` | 20 synthetic contracts with planted clause categories, frozen from `contracts` | exact set of categories |
| `logbook-hard/` | 30 work logs of about 1,650 dated entries each (about 30,000 tokens) with corrections and non-hour distractors, five question shapes, frozen lazily from `logbook` at dial 0.9 | exact rows |
| `memo-rubric/` | 20 synthetic services agreements with an amendment and a rejected proposal; the task is a short memo on three of the terms, frozen lazily from `memo` at dial 0.8 | `judge` with a rubric naming each required fact and a reference memo |
| `coding/` | four Python projects under `workspaces/`, three steps each (implement, extend, fix a bug report); later steps `continues` the earlier step's workspace | the grader's hidden unit tests (`.grader/run.sh N`) |

Rebuild a frozen pack with `kleene bench build <dir> --from <generator>
--count N --dial D --seed S`; the same seeds give the same tasks. With
`--lazy` the pack stores only the generator, dial and seed per task and
regenerates on load, which is how `logbook-hard` and `memo-rubric` are kept
small. Import the real OOLONG with `kleene bench import-oolong
tasks/oolong-trec` (the `trec_coarse` 128k-token split the RLM paper uses,
50 questions, `oolong` oracle; `--dataset`, `--context-len` and `--limit`
pick another slice) and a Harvey LAB checkout with `kleene bench import-lab
<checkout>/tasks tasks/harvey-lab [--sample N --seed S]` (matter folders are
copied into the pack; run with `bench run --outputs <checkout>/results` and
LAB's own evaluator remains the scorer of record). The external datasets
themselves are not redistributed; imported packs live outside git.
