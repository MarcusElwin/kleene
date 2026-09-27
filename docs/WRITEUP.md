# Kleene: a relational calculus for language-model computation

*Status: engineering write-up for the M0–M7 implementation. Every number in
this document comes from the deterministic test suite (scripted providers,
code oracles, replay fixtures); no figure below is a result against a real
model. The measurement design is in place, the measurements are not.*

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

## 4. What the benchmarks will measure

The measurement design from the plan is implemented in `kleene bench`:

- **Learning curve**: a pack run in `learning` mode against the same pack in
  `frozen` mode (no playbook, nothing adopted); accuracy and cost per task
  are rows in `evals`, the rolling mean is `bench curve`.
- **Cost parity**: `plain` mode runs the tool-calling agent on the same
  provider, tools and budgets; accuracy at equal spend is the comparison.
- **Transfer**: learn on a prefix of a pack (`--limit`), then run the rest
  frozen over the same store.
- **Damage control**: `playbook_evals` ties every adopted entry to the replay
  that admitted it; `learn playbook` lists reverts.

Packs shipped: `tasks/terminal` (Terminal-Bench-style), `tasks/oolong-like`
(frozen `corpus` stream), `tasks/finance-synthetic` (frozen `statements`
stream), `tasks/legal-synthetic` (frozen `contracts` stream). Harvey LAB
imports through `bench import-lab` from a checkout; LAB's `evaluation.run_eval`
stays the scorer of record and the in-loop judge oracle approximates it.
The external datasets themselves (OOLONG, Terminal-Bench, FinanceBench,
CUAD, LAB) are not redistributed here.

## 5. Honest gaps

- No results against a real model: the sandbox that built this had no
  provider. Recording fixtures (`bench run --record`) makes the first real
  run replayable for everyone after.
- Threshold calibration from a sample is not built; thresholds are declared.
- Function refinement and the learned cost model persist only in-session
  (sampled selectivity), not as versioned tables.
- The `plain` baseline parses JSON actions from text rather than native tool
  use, which keeps it provider-agnostic but is not identical to a vendor
  agent loop.

## 6. Reproduce

```bash
cargo test --workspace --all-features          # every claim in section 3
cargo run -- repl < demos/planner/three_way.sql # the plan space, live
cargo run -- learn run --tasks 20 --generators puzzle,corpus
cargo run -- bench run tasks/terminal --mode frozen
cargo run -- bench run tasks/terminal --mode plain
cargo run -- bench report
```
