# Task packs

A pack is a directory with a `pack.json`: a name, a description, a license
note, a default task kind and a list of tasks, each with its text, optional
context (inline or a file), optional setup commands and source directory for
a fresh workspace, an optional `check` command that must exit 0 before a
`FINAL` is accepted, an oracle (`exact`, `number`, `sat`, `shell`, `judge`,
`oolong`, `redline`, `human`) and a difficulty prior. `kleene bench run <dir> --mode
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
| `oolong-hard/` | 20 project trackers of about 1,300 dated entries (25,000 tokens) with time in mixed units, action items closed by id, decisions and corrections; five question shapes, frozen lazily from `tracker` at dial 0.9 | exact rows |
| `finance-hard/` | 20 quarterly statements by segment over six years (about 1,100 figures, 16,000 tokens) with restatements and a discontinued segment; totals, growth, ratios, top segment and top quarter, frozen lazily from `ledger` at dial 0.9 | number within tolerance, or exact |
| `legal-hard/` | 20 agreements of about 350 sections (12,000 tokens) with planted categories, a near miss per category, cross-references and amendments; categories present, sections of a category, or a defined-term count, frozen lazily from `agreement` at dial 0.9 | exact rows |
| `terminal-hard/` | eight shell tasks over generated workspaces (log files, CSV joins, duplicates, a rename manifest, a file tree, JSON configs, ledgers, events); `workspaces/<task>/.gen/make.py` builds the files at setup and is removed before the model starts | hidden graders that recompute the answer from the files |
| `recursion-depth/` | 20 reporting trees, five each 3, 4, 5 and 6 levels deep (about 20, 40, 75 and 140 unit reports; 3k to 25k characters), with divested sub-units that drop whole subtrees and `Correction:` rows; one of five questions per depth (consolidated spend of the top or of a sub-unit, the largest sub-unit, units counted, levels), frozen lazily from `nest` at dials 0.4, 0.6, 0.8 and 1.0. The pack states `depth: 6`, so `bench run` lets child sessions nest that deep | number within 0.5, or exact rows |
| `recursion-depth-long/` | the same 20 trees, questions and answers from `nest-long`: every report wrapped in at least 10,000 characters of management commentary (about 200k to 1M characters per task, 50k to 260k tokens), so a deep group's context no longer fits one `llm()` call and has to be partitioned | number within 0.5, or exact rows |
| `recursion-depth-guided/` | the same 20 tasks from `nest-guided`, with one more sentence in the task: that a sub-unit's consolidated spend is the same question one level down, the shape a delegating solution takes | number within 0.5, or exact rows |
| `coding/` | four Python projects under `workspaces/`, three steps each (implement, extend, fix a bug report); later steps `continues` the earlier step's workspace; every step's `check` is `python3 -m unittest -q` | the grader's hidden unit tests (`.grader/run.sh N`) |

Rebuild a frozen pack with `kleene bench build <dir> --from <generator>
--count N --dial D --seed S`; the same seeds give the same tasks. With
`--lazy` the pack stores only the generator, dial and seed per task and
regenerates on load, which is how `logbook-hard`, `memo-rubric`, the
three `*-hard` packs and the three `recursion-depth*` packs are kept small. A pack whose tasks call for child sessions
deeper than the harness default of two states the depth it needs in
`depth`; `bench run` raises its maximum depth to at least that, and
`--max-depth N` overrides either. Import the real OOLONG with `kleene bench import-oolong
tasks/oolong-trec` (the `trec_coarse` 128k-token split the RLM paper uses,
50 questions, `oolong` oracle; `--dataset`, `--context-len` and `--limit`
pick another slice), UmaiTech's contract redlining sets with `kleene bench
import-redlining tasks/redlining-1k [--dataset 1k|10k] [--split test|train]`
(each clause becomes a task with the `redline` oracle holding the reference
redline) and a Harvey LAB checkout with `kleene bench import-lab
<checkout>/tasks tasks/harvey-lab [--sample N --seed S]` (matter folders are
copied into the pack; run with `bench run --outputs <checkout>/results` and
LAB's own evaluator remains the scorer of record). The external datasets
themselves are not redistributed; imported packs live outside git.
