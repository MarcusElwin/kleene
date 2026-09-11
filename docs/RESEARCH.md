# Research digest

Sources behind `docs/PLAN.md`, gathered 2026-09-11. Grouped by the decision
they informed. Where a primary source was unreachable from the build sandbox
the item is marked (snippet).

## Recursive Language Models

- Zhang, Kraska, Khattab. *Recursive Language Models*. arXiv:2512.24601
  (v3 May 2026). https://arxiv.org/abs/2512.24601 — root LM sees only the
  query; context lives as a REPL variable; `llm_query` / `rlm_query` sub-calls;
  experiments at depth 1. OOLONG +114% over GPT-5 at 132k tokens at cost
  parity; BrowseComp-Plus stays at 100% at 1k docs. Ablation: without
  recursion about 90% of full RLM.
- Blog (Oct 2025): https://alexzhang13.github.io/blog/2025/rlm/ — emergent
  strategies: peek, grep, partition-and-map, summarise, solve in code.
- Official repo: https://github.com/alexzhang13/rlm — `RLMLogger` JSONL
  trajectories, visualiser for root/sub/code calls, sandboxes, providers.
  Minimal version: https://github.com/alexzhang13/rlm-minimal
- DSPy `dspy.RLM`: https://github.com/stanfordnlp/dspy/blob/main/dspy/predict/rlm.py
  — `max_iters=20`, `max_llm_calls=50`, `SUBMIT`, no depth > 1.
- Wang. *Think, But Don't Overthink: Reproducing RLMs*. arXiv:2603.02615 —
  depth 2 degrades (format collapse, runaway sub-calls). Drove the depth cap.
- Rust attempts, none faithful to the REPL-variable design:
  https://github.com/zircote/rlm-rs (chunk + BM25/HNSW dispatch),
  https://github.com/joshua-mo-143/rig-rlm (rig + PyO3, early),
  https://github.com/tinyhumansai/tinyagents (graph runtime, GPL-3).

## Long-running and continual harnesses

- Anthropic reference code for long-running agents:
  https://github.com/anthropics/cwc-long-running-agents — feature list,
  progress file, default-FAIL test contract, commit-on-stop hooks, git log as
  audit trail.
- OpenAI *Unrolling the Codex agent loop* and *Harness engineering*
  (snippet): compaction items, sandbox, approvals; Agents SDK `RunState`
  snapshot and rehydrate, tracing spans.
- Curated index: https://github.com/ai-boost/awesome-harness-engineering
- Common pattern across all: append-only event log plus externalised state
  plus checkpoints, re-read on resume. Callgebra makes the state a database.

## Prime Intellect

- Prime Agent (Aug 2026, MIT): https://github.com/PrimeIntellect-ai/prime-agent
  — TypeScript host plus Python kernel, not Rust. Borrowed: one programmatic
  tool with a typed host-request bridge; `spawn` returns a handle and results
  come back as messages; tree-structured JSONL session files with
  `child_usage_attributed` entries; daemon supervisor with generation-aware
  event cursors and snapshot streaming for reconnect; `/tree` fold view;
  budgets and quality gates for autonomous mode; `rlm.harness` ledger with
  `/refine` CRUD edits and rollback.
- verifiers v1: https://github.com/PrimeIntellect-ai/verifiers — Taskset /
  Harness / Runtime / Toolset / Trace separation; an intercepting model proxy
  as the universal trace recorder.
- prime-rl: https://github.com/PrimeIntellect-ai/prime-rl — orchestrator,
  inference and trainer as separate processes; rollouts persisted to the
  filesystem. Relevant only if Callgebra traces are later used for training.

## Headlong

- https://github.com/laude-institute/headlong (Laude Institute, Apache-2.0).
  About 11k lines of Bash, no Rust version exists. `shellm` loop (model writes
  a bash block, Docker runs it, output returns, until `FINAL=`), `traj`
  append-only JSONL DAG with fork/merge, tiered context compaction with a
  tail block kept stable for prompt caches. Trajectory spec:
  https://github.com/laude-institute/headlong/blob/main/design/trajectory_spec.md
- Viewer (headlong-web): step stream coloured by type, collapsible run
  blocks, per-thinker timeline lanes, lazy fork-tree sidebar. These informed
  the TUI's session and trace views.

## TUI references

- Codex CLI (Rust, ratatui): https://github.com/openai/codex/tree/main/codex-rs/tui
  — history cells plus one mutable streaming cell, bottom pane with context
  percentage, status indicator with elapsed timer.
- tau (Rust, ratatui): https://github.com/tau-agent/tau — agent server plus
  thin TUI over a Unix socket, detach and reattach. Closest analogue.
- pi footer (tokens/cost/context), opencode sidebar, crush compact mode
  below 120×30, bottom's tree mode keys, gitui's context-sensitive key bar.
- Crates verified compatible with ratatui 0.30: `tui-tree-widget` 0.24.1,
  `tui-markdown` 0.3.9, `tui-logger` 0.18.3, `tui-scrollview` 0.6.7,
  `ratatui-flow` 0.1.1 (DAG boxes, very new).
- Async pattern: https://ratatui.rs/tutorials/counter-async-app/

## SQL as the LLM interface (prior art)

- LOTUS (VLDB 2025): https://github.com/lotus-data/lotus — semantic
  filter/map/join/agg/topk with proxy cascades and statistical guarantees.
- Palimpzest / Abacus (MIT): https://github.com/mitdbg/palimpzest,
  https://arxiv.org/abs/2505.14661 — Cascades-style cost-based optimiser over
  model and prompt choices; sentinel-plan sampling for cost/quality estimates.
- ThalamusDB: https://github.com/itrummer/thalamusdb — `NLfilter`/`NLjoin`
  over DuckDB with progressive error bounds and call budgets.
- BlendSQL: https://github.com/parkervg/blendsql — `LLMMap`/`LLMQA`/`LLMJoin`,
  AST rewriting to run cheap SQL predicates first, constrained decoding, disk
  cache. Closest in spirit to CallSQL.
- SUQL: https://github.com/stanford-oval/suql — free-text primitives as
  Postgres UDFs with lazy evaluation.
- FlockMTL: https://github.com/dais-polymtl/flock — DuckDB LLM UDFs.
- CAESURA: https://github.com/DataManagementLab/caesura — LLM as planner.
- Evaporate: https://github.com/HazyResearch/evaporate — synthesised
  extraction code versus direct prompting.
- Galois (arXiv:2304.00472): LLM as the database executing plan fragments.
- Scallop / VIERA: foundation models as foreign predicates in Datalog,
  https://dl.acm.org/doi/10.1145/3591280
- Task Cascades (SIGMOD 2026): https://github.com/ucbepic/task-cascades
- Prefix-sharing reordering for LLM SQL workloads: https://arxiv.org/abs/2403.05821
- Benchmarks and surveys: SemBench https://github.com/SemBench/SemBench,
  LRO taxonomy https://arxiv.org/abs/2603.02537

## Complexity results cited in the plan

- Chandra, Merlin 1977: conjunctive query evaluation and containment are
  NP-complete. https://dl.acm.org/doi/10.1145/800105.803397
- Vardi 1982: data vs expression vs combined complexity; relational algebra
  combined complexity PSPACE-complete. https://dl.acm.org/doi/10.1145/800070.802186
- Data complexity of first-order queries is in AC0 (Immerman).
- Datalog: PTIME-complete data complexity, EXPTIME-complete program
  complexity (Dantsin, Eiter, Gottlob, Voronkov 2001).
  https://dl.acm.org/doi/10.1145/502807.502810
- Immerman–Vardi: fixpoint logic captures PTIME on ordered structures.
- Aho, Ullman 1979: relational algebra cannot express transitive closure.
- Gierth, *Cyclic Tag System* in SQL:2003:
  https://wiki.postgresql.org/wiki/Cyclic_Tag_System — the standard evidence
  that `WITH RECURSIVE` makes SQL Turing complete.
- Ibaraki, Kameda 1984: join ordering is NP-complete.
  https://dl.acm.org/doi/10.1145/1270.1498
- Selinger 1979 dynamic programming; Moerkotte, Neumann DPccp 2006 and
  *Dynamic Programming Strikes Back* 2008; Volcano 1993 / Cascades 1995
  memo-based top-down search.

## Hard-instance generation

- Reasoning Gym (NeurIPS 2025): https://github.com/open-thought/reasoning-gym
  — 100+ procedural generators with verifiers and size/depth dials.
- Random 3-SAT phase transition near clause/variable ratio 4.26
  (Mitchell, Selman, Levesque 1992); used to dial LLM difficulty in
  https://arxiv.org/abs/2408.07215
- Graph colouring hardness for LLMs: https://arxiv.org/pdf/2604.01455
- Absolute Zero Reasoner: https://arxiv.org/pdf/2505.03335 — proposer
  rewarded for learnability, executor verifies; the curriculum rule in the plan.
- Proposer/solver collapse when proposer judges itself, and the separate
  verifier fix: survey https://arxiv.org/pdf/2510.27072
- CEGIS (Solar-Lezama) and CEGAR (Clarke et al. 2000): the
  propose / verify / counterexample loop that `VERIFY` + `NOT EXISTS REFUTE`
  expresses relationally.

## Rust crates (versions checked on crates.io, 2026-09-11)

| Crate | Version | Role |
|---|---|---|
| sqlparser | 0.62.0 | parser; recursive CTEs, EXISTS, LATERAL, table functions, visitor feature |
| datafusion | 55.0.0 | rejected for v1 (async UDFs only under projection/filter, plan-time table-function args, heavy build) |
| tokio | 1.53.1 | runtime |
| rusqlite | 0.40.2 | session store, memo, trace, differential oracle |
| ratatui | 0.30.2 | TUI |
| crossterm | 0.29.0 | terminal backend |
| tui-tree-widget | 0.24.1 | call tree |
| tui-markdown | 0.3.9 | streaming output rendering |
| hakoniwa | 1.7.2 | sandbox for `shell` |
| genai | 0.6.5 | considered for multi-provider; not needed while we speak the Messages API directly |
| rig-core | 0.42.0 | considered; heavier abstraction than we want |
| ascent / datafrog | 0.8.1 / 2.0.1 | not needed; semi-naive evaluation is written by hand |
| duckdb | 1.10505.0 | optional trace analytics later |

Notes: no official Anthropic Rust SDK exists (confirmed May 2026). `birdcage`
was archived in July 2026, so it is not used.

## Naming

`callgebra` is taken on npm and GitHub by a small JavaScript library
(https://github.com/fluture-js/callgebra); crates.io and PyPI are free.
`HardQL`, `RLMQL`, `RelationalLM` are free everywhere, but `rllm`
(relationLLM) is a real PyTorch library near the last one.
