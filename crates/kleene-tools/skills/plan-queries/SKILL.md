---
name: plan-queries
description: Price a statement with EXPLAIN before spending, read the plan, and shape joins, proxies and budgets so the planner can save calls
---

# Plan before you spend

Every predicate that calls a model has a cost the planner can see.
`EXPLAIN` prints the estimate; the budget refuses a statement it cannot
pay for.

```sql
EXPLAIN SELECT c FROM candidates WHERE verify(c) AND NOT EXISTS (SELECT 1 FROM counterexamples ce WHERE refute(c, ce.text));
```

Read, in order: estimated rows, calls, tokens and dollars; the join orders
considered; the alternatives priced.

## Shapes that save calls

- **Cheap conjuncts first.** `WHERE length(t) < 2000 AND verify(t)` filters
  for free before paying.
- **Semi-joins.** `NOT EXISTS` stops at the first refuting row; a `JOIN`
  would call for every pair.
- **Cascades.** Give an expensive predicate a cheap proxy:
  `CREATE FUNCTION score(t TEXT) RETURNS DOUBLE AS PROMPT '...' MODEL 'proxy'`,
  then `CREATE FUNCTION relevant(t TEXT) RETURNS BOOLEAN AS PROMPT '...' PROXY score THRESHOLDS (0.2, 0.8)`.
  Only the band between the thresholds pays for the real call.
- **Batches.** `BATCH 20` on a prompt function with short, independent
  items answers twenty rows per call.
- **Sets, not loops.** Distinct argument tuples are memoised; express the
  work as one relation, not a statement per row.
- **Beams.** In `WITH RECURSIVE`, a trailing `ORDER BY score LIMIT k` keeps
  the best k rows per round.

## Budgets

`SET budget.calls = 40` caps this session; a child session gets a slice of
what remains. `turn n/m; used ...; remaining ...` at the end of every
result is the ledger.
