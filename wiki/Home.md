# Kleene

**Relational algebra for recursive model calls.**

Kleene is a harness that runs recursive language-model workflows written in SQL. The model writes **CallSQL**; the engine parses it, annotates every operator with the model and tool calls it implies, prices the plan against a budget, executes it with memoised and concurrent calls, persists tables, memo and trace in DuckDB, and shows it all in a terminal UI.

```sql
SELECT candidate
FROM possibilities
WHERE VERIFY(candidate)
  AND NOT EXISTS (
    SELECT 1 FROM counterexamples ce
    WHERE REFUTE(candidate, ce.text)
  );
```

`VERIFY` is one model call per distinct candidate. The `NOT EXISTS` is an anti-semi-join that stops on the first refuting counterexample. `EXPLAIN` tells you how many calls that is before you spend them.

## Start here

| Page | For |
|---|---|
| [[Installation]] | binaries, Homebrew, the curl installer, building from source |
| [[Quickstart]] | point it at a model, run the first task, read the trace |
| [[Configuration]] | API keys, the config file, routing aliases, web search |
| [[CallSQL Cheat Sheet]] | the dialect on one page: model calls, tools, delegation, `FINAL` |
| [[CLI Reference]] | every `kleene` command and the TUI's slash commands |
| [[Architecture]] | crates, the path of a statement, sessions, store and daemon |
| [[Benchmarks]] | the task packs, the three modes, results so far |
| [[FAQ]] | the questions that come up, with answers |
| [[Contributing]] | working rules, the build, how PRs are organised |

## Three ideas

- **A planner for calls.** Predicates that call a model have a cost the optimizer can see: join order, conjunct order, semi-joins for `EXISTS`, cascades through cheap proxies and beam-limited recursion are rewrite rules with a cost model.
- **Budgets as semantics.** Calls, tokens, dollars and depth are dimensions of a budget that child sessions inherit and slice; a statement the remaining budget cannot pay for is refused with its plan.
- **Learning as tables.** What the harness learns (playbook SQL, ratings, generator dials, sampled selectivities) is rows in DuckDB: inspectable, replay-gated and revertible.

## Elsewhere

- Website: [kleene.sh](https://kleene.sh)
- Repository docs, which this wiki summarises: [`docs/`](https://github.com/MarcusElwin/kleene/blob/main/docs)
- The design and its rationale: [`docs/PLAN.md`](https://github.com/MarcusElwin/kleene/blob/main/docs/PLAN.md)
- The write-up with the claim and what was measured: [`docs/WRITEUP.md`](https://github.com/MarcusElwin/kleene/blob/main/docs/WRITEUP.md)
- Why the name: [README, "Why Kleene"](https://github.com/MarcusElwin/kleene#why-kleene)

Kleene is written in Rust, model-agnostic with no SDK and no gateway required, and MIT licensed. It is built by [Marcus Elwin](https://github.com/MarcusElwin).
