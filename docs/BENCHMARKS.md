# Benchmarks: what `kleene bench` runs and what it measures

This is the one place that explains the benchmark harness end to end: the
four shipped task packs, the three modes every pack runs under, how a task
is judged, what each number in `bench report` means, and what the five
plots show. The commands themselves are listed in
[`docs/CLI.md`](CLI.md#kleene-bench); the claims and the measurement
rationale are in [`docs/WRITEUP.md`](WRITEUP.md); results against a real
model land in the write-up and in the README's
[status section](../README.md#status), not here.

## The question the benchmarks answer

Kleene's claim is that a model writing CallSQL over a relational engine
solves the same tasks as a tool-calling agent for fewer model calls and
dollars, and that the harness gets better over a stream of tasks because
solved SQL is kept as a playbook. The benchmarks are built to test exactly
that, so every run records three things per task: **was it solved**, **how
many model calls did it take**, and **what did it cost**. Everything else
(learning curves, cost parity, transfer) is a query over those rows.

## Packs

A pack is a directory under `tasks/` with a `pack.json`: a name, a one-line
description, a licence note, a default task kind (used to look up playbook
entries) and an ordered list of tasks. Each task has its text, optional
context (inline, or a file relative to the pack), optional setup commands
and a source directory for a fresh workspace, an oracle and a difficulty
prior. The schema is `Pack` and `PackTask` in
`crates/kleene-harness/src/learn/packs.rs`.

| Pack | Tasks | What the model gets | What it must produce | Oracle | Stands in for |
|---|---|---|---|---|---|
| `tasks/terminal` | 6 | an empty or seeded workspace and an instruction (create a file, count lines, rename `.log` to `.txt`, count `TODO` lines, sum a CSV column, write a JSON file) | files left in the workspace and a one-row `FINAL` | `shell`: a command run in the workspace, exit 0 passes | [Terminal-Bench](https://github.com/laude-institute/terminal-bench): shell tasks graded by tests |
| `tasks/oolong-like` | 20 | `ctx`: dated meeting notes, one per row, each naming a project and hours logged, with filler sentences | `FINAL` with one row per project: `project`, `total_hours` | `exact`: the row multiset must match | OOLONG-style long-context aggregation: read everything, group, sum |
| `tasks/finance-synthetic` | 20 | `ctx`: a synthetic income statement, one line item per row, across several fiscal years, with distractor notes | `FINAL` with one row holding a number: a sum over a year, a year-on-year growth percentage, or a ratio | `number`: first cell of the first row within an absolute tolerance (0.5 for sums, 0.15 for percentages, 0.015 for ratios) | [FinanceBench](https://github.com/patronus-ai/financebench)-style open-book numerical QA |
| `tasks/legal-synthetic` | 20 | `ctx`: a synthetic contract, one clause per row, with planted clause categories among boilerplate, paraphrases and near-miss distractors | `FINAL` with one row per category present, out of six named categories | `exact`: the set of categories, spelled as given | CUAD / Harvey LAB-style clause classification |

The three synthetic packs are frozen output of the continual loop's
generators (`corpus`, `statements`, `contracts`) at dial 0.5, seeds 1 to 20.
Freezing means the tasks are checked into the repository, so two runs, or two
people, see the same twenty tasks in the same order; `bench build <dir> --from
<generator> --count N --dial D --seed S` regenerates a pack and the same
arguments give byte-identical tasks. The dial is the generator's hardness knob
in `[0, 1]`: it adds notes and distractors for `corpus`, years, line items and
notes for `statements`, length and paraphrase for `contracts`. The difficulty
prior on every task is derived from the dial (800 rating points at 0, 1600
at 1, so 1200 at 0.5); the terminal tasks carry the 1000 baseline.

The external datasets these packs imitate (OOLONG, Terminal-Bench,
FinanceBench, CUAD, Harvey LAB) are not redistributed. A Harvey LAB checkout
imports with `bench import-lab <root> <out-dir>`; LAB's own evaluator stays
the scorer of record and the in-loop `judge` oracle approximates it.

## Modes

Every pack runs under one of three modes, chosen with `--mode`. The point of
having three is that two comparisons fall out of them.

| Mode | What runs | Playbook shown | Adoption on | Purpose |
|---|---|---|---|---|
| `learning` | Kleene: the model writes CallSQL, the harness executes it | yes, the entries recorded for the task kind | yes: SQL that solved a task becomes a candidate and is adopted after a replay eval wins on solves and cost | the treatment: does the stream get easier as the playbook fills |
| `frozen` | Kleene, same model, same tools, same budgets | no | no: no candidate is recorded, no rating moves | the control for learning: the same abstraction with the playbook off |
| `plain` | a tool-calling agent on the same provider, tools and budgets, no SQL: each turn the model returns one JSON action (`{"tool": …}` or `{"final": …}`) and tool output comes back as text | n/a | n/a | the baseline for cost parity: same model, same spend, only the abstraction differs |

`learning` against `frozen` isolates the effect of the playbook.
`frozen` against `plain` isolates the effect of the SQL abstraction. A
`learning` run also moves the Bradley-Terry ratings of the tasks and the
solver, so `bench plot`'s difficulty axis reflects what the loop has seen.

"Frozen" means frozen for the duration of the run, not empty. Both Kleene
modes read the same store, so a frozen run still uses the memo and the
function definitions the refinement ledger has already adopted (`learn
refine`); what it does not do is show playbook entries, record new
candidates or adopt anything. That is what makes the transfer measurement
below work, and it is why a frozen run on a store that has seen a learning
run is not the same control as a frozen run on a fresh store.

The `plain` agent parses JSON actions from text rather than using a
provider's native tool-calling API. That keeps it provider-agnostic, and is
the one place the baseline is not identical to a vendor agent loop; the
write-up lists it under honest gaps.

Every task, in every mode, runs in its own fresh temporary workspace,
seeded from the pack's `workspace_from` directory and `setup` commands, so
tasks and concurrent runs cannot see each other's files.

## Oracles

A task is solved when its oracle passes on the `FINAL` relation the model
produced. The code oracles are exact and free; the judge oracle costs one
model call and is never the solver's own session. Definitions are in
`crates/kleene-harness/src/learn/verify.rs`.

| Oracle | Passes when |
|---|---|
| `exact` | the rendered rows equal the expected rows as a multiset: cells trimmed, numbers compared numerically, text case-insensitive |
| `number` | the first cell of the first row, with commas and currency signs stripped, is within an absolute tolerance of the expected value |
| `shell` | a command run in the task's workspace, with the answer rows as JSON on stdin, exits 0 |
| `sat` | a 3-SAT verdict is right: `SAT` with a satisfying assignment, or `UNSAT` when brute force agrees |
| `judge` | a separate `judge` model, given a rubric and an optional reference, answers `PASS` |
| `human` | a person marks it; the task waits in `needs_review` and counts as unsolved until then |

The shipped packs use `shell` (terminal), `exact` (oolong-like, legal) and
`number` (finance) only, so none of them needs a judge model to score.

## What is recorded

`bench run` writes one row per task to the `evals` table in the store
(`.kleene/run.duckdb`) and one row per run to `bench_runs`. The `evals`
columns are the raw material for every report and plot:

| Column | Meaning |
|---|---|
| `run` | the run id; one `bench run` invocation |
| `pack`, `mode` | which pack, which of the three modes |
| `seq` | position in the stream, 0-based; the x axis of the learning curve |
| `task`, `kind` | the task's id within the pack and its kind |
| `solved` | the oracle's verdict |
| `detail` | the oracle's reason, for reading failures |
| `calls` | model calls the root session made, children included |
| `tokens` | tokens across those calls |
| `dollars` | cost of those calls at the configured pricing |
| `depth` | the deepest child session spawned (0 for `plain`, which has no children) |
| `turns` | model turns the root took |
| `wall_ms` | wall-clock time |
| `recorded_at` | when the row was written |

`bench csv` prints these rows as CSV for plotting elsewhere. A learning-mode
run also writes to the loop's own tables: `attempts`, `task_ratings`,
`playbook` and `playbook_evals`, which is how `learn playbook` can show what
was adopted after which replay and what was reverted.

## What `bench report` shows

`bench report` groups `evals` by pack and mode over every recorded run:

| Column | Definition |
|---|---|
| `tasks` | rows, so tasks times runs for that pack and mode |
| `accuracy` | fraction of rows with `solved = true` |
| `calls_per_task` | mean `calls` |
| `dollars` | sum of `dollars` |
| `tokens_per_task` | mean `tokens` |
| `depth` | mean `depth` |

Because it aggregates across runs, a pack run twice in the same mode shows
forty tasks, not twenty. Use `bench csv` and filter on `run` for a single
run, or start from a fresh store (`--db` or a new directory) for a clean
comparison.

`bench curve <run-id> [--window 5]` reads one run and prints the rolling
solve rate: at each position the fraction solved over the last `window`
tasks, drawn as a sparkline and listed as numbers. This is the learning
curve of that run.

## The five plots

`bench plot <out-dir>` writes five standalone SVG files from the store,
without a plotting library. Each is the chart the write-up needs for one
claim; a plot with no data says so in the file rather than failing.

| File | Axes | Reads | Claim it supports |
|---|---|---|---|
| `learning_curve.svg` | task number against rolling-5 accuracy, one line per run | a `learning` line that rises while the `frozen` line for the same pack stays flat | the playbook makes later tasks easier |
| `cost_parity.svg` | cumulative dollars against cumulative accuracy, one line per pack and mode | at a given x (equal spend) which mode is higher | Kleene reaches the same accuracy for less money than the plain agent |
| `calls_vs_difficulty.svg` | task rating against calls made, solved and failed as two scatter series | how call counts grow with difficulty, and where failures cluster | the harness spends calls where tasks are hard |
| `estimate_accuracy.svg` | the planner's estimated calls against the calls a statement actually made, from `trace_statements`, with the `y = x` diagonal | points on the diagonal are exact estimates; above it the planner under-estimated | the planner's cost model is trustworthy enough to budget on |
| `plan_space.svg` | relations joined against join orders considered (log scale), one point per statement | how the plan space grows with query shape | the planner's search stays tractable at the shapes models write |

The first three come from `evals`, `attempts` and `task_ratings`; the last
two come from `trace_statements`, so they fill in from any session that ran
SQL through the planner, not only from `bench run`.

## Comparisons the design calls for

The plan names four measurements; this is how each maps onto the commands.

- **Learning curve**: `bench run <pack> --mode learning` and `--mode frozen`
  on the same pack, then `bench curve` on each run id or `bench plot`.
- **Cost parity**: `--mode plain` on the same pack, then `bench report` for
  the totals and `cost_parity.svg` for accuracy at equal spend.
- **Transfer**: `bench run <pack> --mode learning --limit N` to learn on the
  first N tasks, then `bench run <pack> --mode frozen` over the same store,
  and compare the tail against a frozen run on a fresh store. What carries
  over is what the prefix adopted into the store: refined function
  definitions and memo entries. Playbook entries are not shown in frozen
  mode, so the delta measures the learned objects, not few-shot prompting.
- **Damage control**: `playbook_evals` ties every adopted entry to the replay
  eval that admitted it, and `learn playbook` lists reverts, so a regression
  can be traced to the entry that caused it.

## Recording and replaying

`bench run --record <dir>` saves every model reply as a fixture while it
runs against a real provider; `bench run --replay <dir>` serves those
fixtures back, so the same run reproduces offline with no key and no cost.
This is how the first real-model run becomes repeatable for everyone after.
The recording directory is yours to choose and to check in; it is the same
`ReplayProvider` mechanism the test suite uses for its wire-format fixtures.
A replay serves a `frozen` or `plain` run exactly; a `learning` run cannot
be replayed from a frozen run's fixtures, because the playbook it shows
changes the prompt and so the fingerprint, so record each mode separately.

A task that ends in an error without a single token spent means the
provider never answered (no credit, a bad key, a dead endpoint). The run
stops there with the reason in `bench_runs.note` instead of recording the
rest of the pack as failures, and `bench run --resume <run-id>` continues
that run at the first task without a row, keeping the rows before it. The
run must be the same pack and mode.

## Reading results

Results against a real model are reported in
[`docs/WRITEUP.md`](WRITEUP.md) and summarised in the README's
[status section](../README.md#status). When reading them:

- Accuracy on twenty tasks moves in steps of five points; a difference of
  one task is not a result.
- The three synthetic packs are generated, so a solver that learns the
  generator's shape will look better than one meeting the real datasets;
  that is the point of freezing them for the learning curve, and the
  limitation for any absolute claim.
- All three modes share the provider, tools and budget caps, so `dollars`
  is comparable between them. The replay evals a `learning` run pays for
  when it gates a playbook candidate are not in `evals`; they are recorded
  in `playbook_evals` with their own dollars, so the true cost of a learning
  run is the sum of both.

## Adding a pack

Write a `pack.json` following the schema above (or `bench build` from a
generator, or `bench terminal <dir>` for the built-in shell pack), give every
task a stable `id`, a code oracle where one exists, and a `difficulty`
prior, and add a row to [`tasks/README.md`](../tasks/README.md). Anything
that needs a model to grade should use `judge` with a written rubric and a
reference answer, so the verdict is auditable; leave `human` for tasks that
truly need a person.
