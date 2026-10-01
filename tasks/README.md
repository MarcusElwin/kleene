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

Rebuild a frozen pack with `kleene bench build <dir> --from <generator>
--count N --dial D --seed S`; the same seeds give the same tasks. Import
the real OOLONG with `kleene bench import-oolong tasks/oolong-trec` (the
`trec_coarse` 128k-token split the RLM paper uses, 50 questions, `oolong`
oracle; `--dataset`, `--context-len` and `--limit` pick another slice) and a
Harvey LAB checkout with `kleene bench import-lab <root> tasks/harvey-lab`
(matter folders are copied into the pack; LAB's own evaluator remains the
scorer of record). The external datasets themselves are not redistributed;
imported packs live outside git.
