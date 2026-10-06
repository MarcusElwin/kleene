# Benchmarks

What `kleene bench` runs and what it measures. The full explanation is [`docs/BENCHMARKS.md`](https://github.com/MarcusElwin/kleene/blob/main/docs/BENCHMARKS.md); the reading of the results is in [`docs/WRITEUP.md`](https://github.com/MarcusElwin/kleene/blob/main/docs/WRITEUP.md#4-what-the-benchmarks-measured); the live table and charts are on [kleene.sh/benchmarks](https://kleene.sh/benchmarks/).

## The question

Kleene's claim is that a model writing CallSQL over a relational engine solves the same tasks as a tool-calling agent for fewer model calls and dollars, and that the harness gets better over a stream of tasks because solved SQL is kept as a playbook. So every run records three things per task: **was it solved**, **how many model calls did it take**, and **what did it cost**.

## Packs

A pack is a directory under `tasks/` with a `pack.json`. The shipped packs are synthetic or hand-written; the external datasets they imitate are not redistributed.

| Pack | Tasks | Shape | Oracle | Stands in for |
|---|---|---|---|---|
| `terminal` | 6 | shell tasks in a workspace | `shell` | Terminal-Bench |
| `oolong-like` | 20 | aggregate hours per project over sixty meeting notes | `exact` | OOLONG |
| `finance-synthetic` | 20 | a number from a synthetic income statement | `number` with tolerance | FinanceBench |
| `legal-synthetic` | 20 | clause categories present in a synthetic contract | `exact` | CUAD, Harvey LAB |
| `logbook-hard` | 30 | questions over a 118,000-character work log with corrections | `exact` | OOLONG at a size that cannot be read once |
| `memo-rubric` | 20 | a short memo on a 40-section agreement with a superseding amendment | `judge` against a rubric | Harvey LAB drafting |
| `coding` | 12 | implement, extend, then fix a module in a small Python project, in three-step episodes | `shell` over hidden tests | SWE-bench |

Three more import on demand: `bench import-oolong` (OOLONG from Hugging Face), `bench import-lab` (a Harvey LAB checkout, 2,010 tasks) and the redlining packs.

## Modes

| Mode | What runs |
|---|---|
| `learning` | Kleene with the playbook shown and adopted; solved SQL is kept after a replay-gated eval |
| `frozen` | Kleene with the playbook off: the control |
| `plain` | a one-call tool-calling agent on the same provider, tools and budget: the baseline for cost parity |

## Running one

```bash
kleene bench run tasks/terminal --mode frozen        # one pack, one mode
kleene bench run tasks/memo-rubric --mode learning --record fixtures/memo   # record replies for replay
kleene bench report                                   # accuracy and cost per pack and mode
kleene bench results plots/evals.csv --readme README.md   # regenerate the README table and plots
```

Every task lands in the `evals` table; `bench run --resume` continues a stopped run, and a provider error aborts the run rather than recording a failure.

## Results so far

Two real-model runs. Claude Opus 5.5 on 27 September 2026 solved every task of the four original packs in every mode, so those packs compare cost only: the playbook cuts calls per task (oolong 4.9 to 3.9, legal 12.7 to 10.6), and the one-call plain agent is cheapest wherever the whole context fits in a prompt. Claude Haiku 4.5 on 1 October 2026, with Claude Sonnet 5.5 as the judge, separates on accuracy on the harder packs: 2 of 12 coding steps, and on `memo-rubric` 11 of 20 learning against 8 of 20 frozen and 8 of 20 plain. The per-task rows are under [`plots/`](https://github.com/MarcusElwin/kleene/blob/main/plots) and the README's results table is generated from them.
