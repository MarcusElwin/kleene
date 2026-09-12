# Planner demos

Two scripts for `callgebra repl` (pipe them in). The `EXPLAIN`s need no
provider; the `SELECT`s do.

- `three_way.sql`: a three-way join with two LLM predicates written as a
  cross product. `EXPLAIN` prints the plan space (3 relations, 12 bushy
  orders), the chosen order and the as-written cost beside it: the naive plan
  asks the model for every triple, the chosen one filters `papers` first and
  asks `supports` only for matched (claim, evidence) pairs. A seven-way join
  follows: 665,280 orders, priced through at most 1,932 subset splits.
- `cascade_and_beam.sql`: an oracle predicate with a declared proxy becomes a
  cascade (`score >= high OR (score >= low AND oracle)`), and a recursive
  hypothesis search with `ORDER BY ... LIMIT 5` becomes a beam of width five.

In the TUI, view 2 (plan) shows the same `EXPLAIN` text for the selected
statement, with the alternatives and plan-space lines.
