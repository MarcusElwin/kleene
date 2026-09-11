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

Written in Rust. The name is also used by an unrelated small JavaScript
library (`fluture-js/callgebra`).
