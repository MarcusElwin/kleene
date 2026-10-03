# Kleene: a relational calculus for language-model computation

*Status: engineering write-up for the M0–M7 implementation. Section 3 is
the deterministic test suite (scripted providers, code oracles, replay
fixtures). Section 4 is the first run of the four shipped packs against a
real model (Claude Opus 5.5, 27 September 2026); the per-task rows are
`plots/evals.csv`.*

## 1. Claim

An agent's work can be written as queries. Retrieval is a scan or a `grep`
table function, judgement is a boolean call function in a `WHERE`, delegation
is a lateral table call that opens a child session, search is a recursive CTE
with a beam `LIMIT`, and the answer is a relation. Once the work is a query,
three things fall out that a tool-calling loop cannot offer:

- **A planner.** Call predicates have a cost the optimizer can see, so join
  order, conjunct order, semi-joins for `EXISTS`, cascades through cheap
  proxies and beam-limited recursion are rewrite rules with a cost model,
  not prompt engineering. `EXPLAIN` shows the estimate before the spend.
- **Budgets as query semantics.** Calls, tokens, dollars, depth and wall
  clock are dimensions of a `Budget` inherited and sliced by child sessions;
  a statement whose estimate exceeds what is left is refused with its plan.
- **Learning as tables.** What the harness learns (playbook SQL, ratings,
  generator dials, sampled selectivities) is rows in DuckDB, inspectable,
  replay-gated and revertible.

## 2. The algebra

Ordinary operators (σ, π, ⋈, ∪, γ, μ) plus a *call kind* on any operator that
evaluates a call expression: λ_f (scalar map-call, one call per distinct
input), κ_g (expand-call, `LATERAL g(...)`, output cardinality the branching
factor), ρ_d (`rlm` / `spawn`, the child's whole plan), and σ_llm / ⋈_llm
(predicates containing calls). Cost is calls × per-alias tokens and dollars;
selectivity comes from observed pass rates once a predicate has been seen.

The eight rewrite rules of the plan are implemented: cheap-first over filters
and join conditions, memo dedupe, batching (`BATCH n` on a prompt function:
one call answers up to n distinct tuples, priced as `ceil(rows / n)` calls),
cascade, semi-join, beam-limited recursion, join ordering with
branch-and-bound over relation subsets, and volatility fences. Estimates mirror the executor's left-to-right
short-circuit evaluation, so cheap-first is visible in the numbers rather
than assumed.

The complexity fragment of every statement is reported (CQ / FO / REC) as a
badge; it is a reminder that a recursive call plan is a program, not a
lookup.

## 3. What was measured, deterministically

| Property | Test | Result |
|---|---|---|
| Relational core equals DuckDB | property-based differential suite | passes, recursive CTEs included |
| Join ordering with two call predicates over three relations | estimate and actual calls against a counting sink | as written 12,000 estimated calls / chosen 60; actual ≥10× fewer with identical rows |
| Plan-space growth | seven-way join | 665,280 bushy orders priced through 1,623 subset splits (309 pruned) |
| Cascade | proxy at 0.1× cost, thresholds (0.2, 0.8) | 100 proxy + 60 oracle calls beat 100 oracle calls by estimate; on data the oracle is asked only for the band (5 of 20) |
| Beam recursion | `expand` × 4, three rounds | 20 calls with beam 5 against >100 unbounded |
| Continual loop | scripted solver over generated puzzles | tasks solved, playbook v1 adopted after a replay eval, ratings and dial move, restart resumes |
| Terminal-Bench-style pack | shell tasks from SQL | `write_file` and `lines` tasks solved, oracles run in fresh workspaces |
| Plain-agent baseline | same provider, JSON actions | tool call then final; the control for cost parity |

## 4. What the benchmarks measured

The four shipped packs ran once each under the three modes of
[`docs/BENCHMARKS.md`](BENCHMARKS.md) on 27 September 2026, with the default
routing: `root` on `claude-opus-5-5` at high effort, `worker` and `judge` on
`claude-sonnet-5`, `proxy` on `claude-haiku-4-5`, priced at the list rates
in the router. The per-task rows (`kleene bench csv`) are `plots/evals.csv`
and the table is `kleene bench report` (`plots/report.txt`) over the store
those runs left; the model replies were recorded but are not checked in,
because the catalog issue below means they cannot replay the runs yet.
`dollars` is the
pack total, `calls` and `tokens` are per task, and the learning column
excludes the gate replays (below).

| pack | mode | solved | calls / task | tokens / task | dollars |
|---|---|---|---|---|---|
| terminal (6) | frozen | 6/6 | 2.17 | 10,551 | 0.07 |
| terminal (6) | learning | 6/6 | 1.33 | 6,742 | 0.04 |
| terminal (6) | plain | 6/6 | 2.50 | 2,395 | 0.03 |
| oolong-like (20) | frozen | 20/20 | 4.90 | 30,778 | 1.51 |
| oolong-like (20) | learning | 20/20 | 3.85 | 41,991 | 1.92 |
| oolong-like (20) | plain | 20/20 | 1.00 | 3,084 | 0.39 |
| finance-synthetic (20) | frozen | 20/20 | 3.45 | 42,177 | 0.83 |
| finance-synthetic (20) | learning | 20/20 | 2.40 | 29,999 | 0.63 |
| finance-synthetic (20) | plain | 20/20 | 1.00 | 1,244 | 0.07 |
| legal-synthetic (20) | frozen | 20/20 | 12.65 | 62,452 | 1.69 |
| legal-synthetic (20) | learning | 20/20 | 10.60 | 58,315 | 2.61 |
| legal-synthetic (20) | plain | 20/20 | 1.00 | 1,475 | 0.11 |

What the numbers say, read with the caveats of
[`docs/BENCHMARKS.md`](BENCHMARKS.md#reading-results):

- **Accuracy is saturated.** Opus 5.5 solves every task of every pack in
  every mode. The packs were sized for a weaker model and a dial of 0.5;
  they separate nothing on accuracy, so the comparison below is on calls
  and dollars only. Harder dials, longer contexts and real datasets are the
  next step.
- **The playbook cuts calls per task.** Learning against frozen: terminal
  2.17 to 1.33, oolong 4.90 to 3.85, finance 3.45 to 2.40, legal 12.65 to
  10.60. On oolong the second half of the learning run took 3.9 calls per
  task where the first half took 3.8, so most of the gain arrived with the
  first adopted entries; on legal the halves are 13.7 and 7.5, the curve
  the design predicts. Only oolong (16 of 20 candidates) and legal (5 of 21)
  adopted anything: on terminal and finance every candidate was rejected by
  the gate because its replay cost more than the baseline's, so their
  learning runs are frozen runs with ratings on, and the call reduction
  there is run-to-run variance, not learning.
- **The gate is the cost.** Each candidate is replayed on three earlier
  tasks in both arms, six task runs per solved task. Those replays are
  recorded in `playbook_evals`, not `evals`: they cost $1.25 on terminal,
  $10.70 on oolong, $5.46 on finance and $16.63 on legal against $0.04,
  $1.92, $0.63 and $2.61 for the counted runs. At twenty tasks a pack the
  gate spends far more than the playbook saves; it earns its keep only over
  a long stream, and a cheaper gate (a smaller sample, a proxy model, a
  cached baseline) is the obvious next change.
- **The plain agent wins on cost here.** Every context in these packs fits
  in one prompt (under 4,000 characters), so the tool-calling baseline
  answers in one call with no tool use: $0.39 against $1.51 on oolong, $0.07
  against $0.83 on finance, $0.11 against $1.69 on legal. The SQL abstraction
  pays for a catalog, a planner and a transcript of rendered results on
  every turn, and at this scale that is pure overhead. The claim of section
  1 is about work that does not fit in one call; these packs do not test
  it, and the numbers say so.
- **Per-clause calls are cheap and visible.** On legal, the model classified
  clause by clause with a prompt function on the `worker` alias (up to 26
  calls a task at about $0.10), which is the λ_f map-call of section 2
  doing what it is for: many small calls on a cheap model, priced before
  they run.
- **Costs drift up within a run.** Per-task dollars on oolong learning rose
  from $0.084 in the first half to $0.107 in the second while calls stayed
  flat. Two things grow the prompt: the adopted playbook entries, and the
  scratch tables every session creates in the shared store, which the
  catalog lists to every later session (287 such tables after these runs).
  The second is a bug: it also changes the prompt fingerprint from task to
  task, so a recorded run cannot be replayed from its own fixtures until
  session tables are scoped or dropped.

The other measurements of the design are in place but not yet exercised by
these runs: transfer (learn on a prefix with `--limit`, then run the rest
frozen), reverts (`learn playbook`), and the estimate and plan-space plots,
which read `trace_statements` and were empty because these runs were made
before `bench run` traced statements (it does now).

Packs shipped: `tasks/terminal` (Terminal-Bench-style), `tasks/oolong-like`
(frozen `corpus` stream), `tasks/finance-synthetic` (frozen `statements`
stream), `tasks/legal-synthetic` (frozen `contracts` stream). Harvey LAB
imports through `bench import-lab` from a checkout; LAB's `evaluation.run_eval`
stays the scorer of record and the in-loop judge oracle approximates it.
The external datasets themselves (OOLONG, Terminal-Bench, FinanceBench,
CUAD, LAB) are not redistributed here. A coding pack (edit code in a
workspace, judged by its tests) is the next pack to add.

## 5. Honest gaps

- One model, one run per pack, and every task solved: the results in
  section 4 compare cost, not accuracy, and need harder packs and a second
  model before they say more.
- Session scratch tables persist in the shared store and appear in every
  later session's catalog, which grows prompts over a run and breaks
  replaying a recorded run from its own fixtures.
- The replay gate costs six task runs per solved task and dominates the
  cost of a learning run at pack scale.
- Threshold calibration from a sample is not built; thresholds are declared.
- The learned cost model persists only in-session (sampled selectivity), not
  as versioned tables.
- The `plain` baseline parses JSON actions from text rather than native tool
  use, which keeps it provider-agnostic but is not identical to a vendor
  agent loop.

## 6. Reproduce

```bash
cargo test --workspace --all-features          # every claim in section 3
cargo run -- repl < demos/planner/three_way.sql # the plan space, live
cargo run -- learn run --tasks 20 --generators puzzle,corpus
cargo run -- bench run tasks/terminal --mode frozen --record fixtures/terminal
cargo run -- bench run tasks/terminal --mode plain --record fixtures/terminal-plain
cargo run -- bench report
cargo run -- bench plot plots/
```

The runs of section 4 are `plots/evals.csv` and `plots/report.txt`, with
the SVGs `bench plot` drew from the same store beside them. `fixtures/` is
ignored by git: record your own with `--record`, and replay with `--replay`
once the catalog caveat in section 5 is fixed.
