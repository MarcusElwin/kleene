# Benchmarks: what `kleene bench` runs and what it measures

This is the one place that explains the benchmark harness end to end: the
seven shipped task packs, the three modes every pack runs under, how a task
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
| `tasks/logbook-hard` | 30 | `ctx`: a work log of about 1,650 dated entries (about 118,000 characters, 30,000 tokens) in four phrasings, with entries about the same projects whose numbers are not hours, and `Correction:` entries that amend an earlier entry | `FINAL` answering one of five questions: hours per project, hours per person on one project, the top person, hours per project in one month, distinct people on a project | `exact` | OOLONG at a size that cannot be read once and answered: the context has to be filtered and aggregated |
| `tasks/memo-rubric` | 20 | `ctx`: a synthetic services agreement of about 40 sections with six planted commercial terms, a rejected proposal in a schedule, and an amendment that supersedes one term | `FINAL` with one row holding a short written memo answering three questions about the terms | `judge`: a separate model grades the memo against a rubric naming each required fact and forbidding the superseded and rejected values, with a reference memo | Harvey LAB-style drafting graded by rubric |
| `tasks/coding` | 12 | a small Python project in the workspace (four projects: a finance CSV library, word statistics, warehouse stock, day-interval scheduling), visible unit tests, and a task: implement a module, then extend it, then fix a bug report | files edited in place and a one-row `FINAL` | `shell`: the grader's own hidden unit tests for every step so far (visible tests plus edge cases the task text states) | SWE-bench-style code editing, in three-step episodes over one checkout |

The synthetic packs are frozen output of the continual loop's generators:
`corpus`, `statements` and `contracts` at dial 0.5, seeds 1 to 20, and the
harder `logbook` (dial 0.9, 30 seeds) and `memo` (dial 0.8, 20 seeds).
Freezing means the tasks are checked into the repository, so two runs, or two
people, see the same tasks in the same order; `bench build <dir> --from
<generator> --count N --dial D --seed S` regenerates a pack and the same
arguments give byte-identical tasks. The first three packs are written out in
full; `logbook-hard` and `memo-rubric` are built with `--lazy`, so their
`pack.json` holds a generator, dial and seed per task (`from`) and the task
is regenerated on load. The two forms load to the same task, because every
generator is a pure function of its dial and seed; the lazy form keeps a
pack with 118,000-character contexts at a few kilobytes on disk. The dial is
the generator's hardness knob in `[0, 1]`: it adds notes and distractors for
`corpus`, years, line items and notes for `statements`, length and
paraphrase for `contracts`, entries, phrasings, distractors and corrections
for `logbook`, and length and the amendment for `memo`. The difficulty prior
on every task is derived from the dial (800 rating points at 0, 1600 at 1, so
1200 at 0.5); the terminal tasks carry the 1000 baseline and the coding
steps 1150 to 1350.

The coding pack is hand-written. Each project is a directory under
`tasks/coding/workspaces/` copied into the workspace, and its three steps
form an **episode**: the second and third steps carry `continues` naming the
step before, so they run in the workspace that step left, files and all,
rather than in a fresh copy. A step's `setup` runs in the inherited
workspace; the coding steps use it to check the previous step with the
grader's hidden tests and, when they fail (the earlier step was not solved,
or the run was resumed and the workspace is gone), to install the reference
solution for that step, so each step is measured on its own work. The
grader (`.grader/run.sh N`) runs its own copies of the tests for steps 1 to
N, so editing `tests/` changes nothing; the visible tests under `tests/` are
a subset, and the edge cases the hidden tests add are all stated in the task
text.

The external datasets these packs imitate (OOLONG, Terminal-Bench,
FinanceBench, CUAD, Harvey LAB) are not redistributed. Three import on
demand:

- **OOLONG** (Bertsch et al. 2025, MIT): `bench import-oolong <out-dir>`
  downloads questions from the Hugging Face dataset `oolongbench/oolong-synth`
  and writes a pack with the `oolong` oracle. The default is the
  `trec_coarse` source dataset at the 131,072-token bucket: 50 questions (33 counting,
  17 per-user; comparisons, counts, labels and user ids) over two shared
  context windows of about 3,200 TREC questions each, which is the split the Recursive Language Models paper reports on.
  `--dataset`, `--context-len`, `--limit` and `--offset` pick another slice
  (`spam` is the other validation dataset; `agnews`, `app_reviews`,
  `formality`, `imdb`, `metaphors`, `multinli`, `negation` and `yahoo` are
  the test datasets; buckets run from 1,024 to 4,194,304 tokens). Every
  context window is written once to `contexts/` and shared by its questions,
  one line of the original per `ctx` row, so the pack is self-contained and
  the same arguments give the same pack. The importer reads each parquet
  shard's footer with a range request to find the row groups that hold the
  slice and fetches only those rows, so a 12 GB dataset costs a few tens of
  megabytes to import. `--from-json <file>` builds the pack from rows saved
  earlier instead of the network.
- **Harvey LAB** (MIT, `harveyai/harvey-labs`): a checkout imports with
  `bench import-lab <checkout>/tasks <out-dir>`, all 2,010 tasks (1,599
  standalone plus 411 workflow scenarios) in about twenty seconds. Each
  task's `documents/` folder is copied in as its workspace; the 250
  firm-knowledge tasks that share one `dms` corpus through `docs_dir` share
  one copy. The oracle is `judge` over the task's rubric exactly as shipped:
  one line per criterion with its id, title, `match_criteria` and the
  deliverables it is scoped to, all-pass. LAB's own evaluator
  (`lab_core.evaluation.run_eval`, Sonnet 4.6 and GPT-5.5 as judges) stays
  the scorer of record; the in-loop oracle approximates it with one judge
  and no per-deliverable scoping. The matter documents are `.docx`,
  `.xlsx`, `.pptx` and `.eml`, which `read` returns as text (paragraphs
  and tables, one CSV block per sheet, one block per slide, decoded mail),
  and a deliverable written with `CALL write_file` to a `.docx` or `.xlsx`
  path is built from the Markdown or CSV given (through `pandoc` when it
  is installed, else a minimal package), so LAB's evaluator can open it.

  To score with LAB's evaluator, run with `--outputs <checkout>/results`:
  every task's `output/` is exported as
  `results/<task>/kleene-<mode>/<run>/output/` with the `config.json` and
  `metrics.json` LAB's reports expect, and `bench run` prints one
  `run_eval` command per task. Sampling keeps the cost of a first pass
  down: `import-lab … --sample 50 --seed 1` imports a seeded sample (and
  copies only its documents), `bench run … --sample 20 --seed 1` runs a
  seeded sample of any pack, in pack order, so two people with the same
  seed run the same tasks. One practice area
  (`import-lab <checkout>/tasks/antitrust-competition …`, 33 tasks) is the
  other cheap start.
- **Contract redlining** (UmaiTech, CC BY 4.0): `bench import-redlining
  <out-dir>` downloads examples from `UmaiTech/legal-contract-qpt5-redlining-1k`
  (`--dataset 1k`, 992 redlines by a GPT-5 model mix, the default) or
  `UmaiTech/legal-contract-gpt41-redlining-10k` (`--dataset 10k`, GPT-4.1),
  synthetic client-protective redlines of clauses from the CUAD contracts:
  ten contract types, ten US jurisdictions, the liability, termination,
  warranty, IP and governing-law categories. The default is the held-out
  `test` split (10%, 100 examples of the 1k set), read from the `alpaca`
  config; `--split train`, `--limit` and `--offset` pick another slice and
  `--from-json <file>` builds the pack from rows saved earlier. Each
  example becomes one task: the task text names the clause category,
  contract type, jurisdiction and expected risk reduction and quotes the
  original clause, and asks for `FINAL` over one row with `redline` (the
  full revised clause) and `rationale`. The oracle is `redline`, which holds
  the reference redline, its rationale and the specific changes it lists
  (below). The reference is itself model-written, so the pack measures
  agreement with a GPT redline, not with a lawyer; it is the dataset's own
  framing and it is cheap enough to run the whole test split.

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
write-up lists it under honest gaps. A reply that holds several JSON
objects (Haiku 4.5 writes five to ten actions per reply, with imagined
tool output between them) has only its first object run; the real output
of that one action comes back as the next user turn.

Every task, in every mode, runs in its own fresh temporary workspace,
seeded from the pack's `workspace_from` directory and `setup` commands, so
tasks and concurrent runs cannot see each other's files; the exception is a
task that `continues` another, which inherits that task's workspace (see the
coding pack above). Every bench session also drops the tables it created
(`ctx`, its `CREATE TABLE` scratch, a child's namespace) when it finishes,
so the catalog a session sees does not grow with every session before it and
a recorded run replays from its own fixtures (`drop_session_tables` in the
harness configuration; a REPL keeps its tables).

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
| `oolong` | an OOLONG answer, scored as the paper scores it: a label, date, user id or comparison on exact match (any of the listed values when the dataset lists a tie), a number with partial credit `0.75^|expected − got|`. The task passes only at a score of 1; the score itself is in `detail` (`oolong score 0.562: expected 1542, got 1540`), so a run's mean OOLONG score is a query over `evals.detail` |
| `redline` | a contract redline: the `redline` cell of the first row must differ from the original clause (an unchanged clause fails outright), and it is scored on `redline recall`, the share of the reference redline's new terms (words of four letters or more that the original lacks) the answer carries. With a judge configured the judge decides, given the original clause, the reference redline with its rationale and the specific changes it lists, and `detail` carries both (`redline recall 0.83; judge: PASS …`); without one the task passes at a recall of 0.50, so an imported redlining pack can be run for free at the cost of a lexical oracle |
| `human` | a person marks it; the task waits in `needs_review` and counts as unsolved until then |

The shipped packs use `shell` (terminal, coding), `exact` (oolong-like,
legal, logbook-hard) and `number` (finance); only `memo-rubric` uses
`judge`, which sends the rubric, the reference and the answer's cells in
full to the `judge` alias (Sonnet in the default routing) and costs one
short call per task. The judge is asked for `PASS` or `FAIL` on its first
line; a first line that says PASS but also mentions FAIL ("PASS or FAIL?",
"PASS (provisional), though it would FAIL on ...") counts as a fail, since
a hedged pass is not a verdict. A rubric-judged verdict is as good as its
rubric: the memo rubrics name every fact the memo must state, with its
number, and forbid the superseded and rejected values, so the judge's job
is checking, not appraising. An imported OOLONG pack uses `oolong`, which
needs no judge either; an imported redlining pack uses `redline`, which
uses the judge when there is one and its recall score when there is not.

## What is recorded

`bench run` writes one row per task to the `evals` table in the store
(`.kleene/run.duckdb`) and one row per run to `bench_runs`. The `evals`
columns are the raw material for every report and plot:

| Column | Meaning |
|---|---|
| `run` | the run id; one `bench run` invocation |
| `pack`, `mode` | which pack, which of the three modes |
| `model` | the model the `root` alias resolved to when the row was written (NULL under the replay and test providers, which cannot name one) |
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

`bench report` groups `evals` by pack, mode and model over every recorded run:

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

## Results across models

Runs on different models usually live in different stores (one per
machine, per day, per pack and mode), so the comparison across models
starts from their `bench csv` exports rather than from a merged database:

```bash
kleene bench results plots/evals.csv plots/haiku-2026-10-01/evals.csv \
  --out plots/results --readme README.md \
  --plot coding --plot memo-rubric --plot logbook-hard
```

reads every CSV given, groups the rows by pack, mode and `model`, and
writes to `--out`:

- `results.md`: the summary table, one row per pack, mode and model with
  the metrics as columns (tasks, pass rate as `solved/tasks (percent)`,
  mean dollars per task, total dollars, mean calls, mean tokens and mean
  seconds per task), followed by one collapsible `<details>` block per
  plotted pack holding its plot. A model that has not been run on a pack
  in a mode keeps its row with blank metrics, not zeros, so the table can
  be published before every model has run every pack.
- `<pack>-pareto.svg`: dollars per task against pass rate, one point per
  model and mode (one colour per model, the mode written at the point)
  and a dashed line through the Pareto frontier, the points no other point
  beats on both axes. Reading it: a point on the line is a defensible
  choice at its budget; a point below and to the right of the line is
  dominated by one that is both cheaper and more often right.

`--plot <pack>` (repeatable) limits which packs get a `<details>` block
in the Markdown; every SVG is still written. The README shows the hard packs only: on the
four original packs every mode scored the same, so their points sit on one
horizontal line and the table says it all.

With `--readme`, the Markdown also replaces the block between
`<!-- bench-results:begin -->` and `<!-- bench-results:end -->` in that
document, with image links relative to it; the README's results section
is maintained this way, so a new model is a new CSV and a re-run of the
command, never a hand-edited table. Models are shown by a readable form
of their id (`claude-opus-5-5` as Claude Opus 5.5, a trailing date
dropped) and the mapping is printed under the table.

The `model` column is written by `bench run` from the provider's answer for
the `root` alias (the router's first candidate). CSVs exported before the
column existed need it added by hand, as `plots/evals.csv` (Opus 5.5) and
`plots/haiku-2026-10-01/evals.csv` (Haiku 4.5) were; the command refuses a
CSV without it rather than guessing.

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
  on the same pack, then `bench curve` on each run id or `bench plot`. Use
  a fresh store (`--db`) for each mode: both Kleene modes read and write
  the same memo, so the second run in a store answers every prompt the
  first run already sent for free, and its calls and dollars are not an
  independent measurement (two coding steps in the Haiku learning run
  finished with zero calls this way). One store per pack and mode also
  lets the runs go in parallel, since DuckDB has one writer per file; the
  rows merge afterwards with `kleene trace "ATTACH 'other.duckdb' AS o
  (READ_ONLY); INSERT INTO evals SELECT * FROM o.evals"` (and the same for
  `bench_runs`, `attempts`, `playbook_evals`, `trace_statements`, `tasks`,
  `task_ratings`), after which `bench report` and `bench plot` read the
  merged store.
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
- Pace the long-context pack to the model. On Haiku 4.5 a `logbook-hard`
  task in frozen mode took some 500 calls and over twenty minutes, so 30
  tasks in three modes is more than a day; run it with `--limit 10` and
  compare the same ten tasks across modes.
- All three modes share the provider, tools and budget caps, so `dollars`
  is comparable between them. The replay evals a `learning` run pays for
  when it gates a playbook candidate are not in `evals`; they are recorded
  in `playbook_evals` with their own dollars, so the true cost of a learning
  run is the sum of both.

## Adding a pack

Write a `pack.json` following the schema above (or `bench build` from a
generator, with `--lazy` for long contexts, or `bench terminal <dir>` for
the built-in shell pack), give every task a stable `id`, a code oracle where
one exists, and a `difficulty` prior, and add a row to
[`tasks/README.md`](../tasks/README.md). A task may instead carry `from`
(generator, dial, seed) and be regenerated on load, with any of the other
fields as overrides. Steps of an episode name the step before in
`continues`, which must be an earlier task of the pack; their `setup` runs
in the inherited workspace, and a pack whose steps can repair a missing
predecessor there (as the coding pack does from its reference solutions)
resumes cleanly with `--resume`. Anything that needs a model to grade should
use `judge` with a written rubric and a reference answer, so the verdict is
auditable; leave `human` for tasks that truly need a person.
