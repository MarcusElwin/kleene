# callgebra

**Callgebra — relational algebra for recursive model calls.**

Write declarative SQL. Compile joins, recursion, predicates and aggregation
into an execution graph of language-model calls, recursive sub-sessions and
tool calls. Plan and cost that graph, run it, and query the trace with the
same SQL. Watch it happen in a terminal UI.

```sql
SELECT candidate
FROM possibilities
WHERE VERIFY(candidate)
  AND NOT EXISTS (
    SELECT 1 FROM counterexamples ce
    WHERE REFUTE(candidate, ce.text)
  );
```

`VERIFY` is one model call per distinct candidate. The `NOT EXISTS` is an
anti-semi-join that stops on the first refuting counterexample. `EXPLAIN`
tells you how many calls that is before you spend them.

Status: **planning**. Read [`docs/PLAN.md`](docs/PLAN.md) for the
architecture, dialect, planner rules, harness design, TUI layout and
milestones, and [`docs/RESEARCH.md`](docs/RESEARCH.md) for the sources.

Written in Rust. Model-agnostic with no SDK and no gateway required: two
thin adapters (Anthropic native, OpenAI-compatible) behind one `Provider`
trait, an alias router with failover and pricing, and an optional Open
Responses gateway adapter for gateways such as
[Aura](https://github.com/UmaiTech/aura-llm-gateway). Sub-work is delegated to declared agent roles
with `CREATE AGENT` and `SPAWN`. Everything the model does is SQL: shell,
file edits, web and memory are `CALL` statements with volatility classes the
planner respects. Session tables, memo and trace live in one embedded DuckDB
file.

The name is also used by an unrelated small JavaScript library
(`fluture-js/callgebra`).
