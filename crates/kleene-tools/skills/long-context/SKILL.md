---
name: long-context
description: Answer a question over a context too large for one prompt by peeking, partitioning, mapping child sessions with rlm, and reducing
---

# Long context

The context is a table, `ctx(ordinal, text)`. Never select it whole.

1. **Peek.** `SELECT COUNT(*), MIN(ordinal), MAX(ordinal) FROM ctx;` and a
   `LIMIT 3` sample.
2. **Locate** what matters cheaply: `WHERE text ILIKE '%hours%'`, or
   `grep` over files, or `search` for a concept.
3. **Partition.** Group rows into chunks a child can hold:
   `CREATE TABLE parts AS SELECT ordinal / 20 AS part, string_agg(text, '\n' ORDER BY ordinal) AS chunk FROM ctx GROUP BY 1;`
4. **Map.** One child session per partition, concurrently:
   `CREATE TABLE answers AS SELECT p.part, r.answer, r.detail FROM parts p CROSS JOIN LATERAL rlm('Which project consumed the most hours, and how many?', p.chunk) r;`
5. **Reduce.** Aggregate the children's answers with SQL, or one more
   `llm_json` over the small `answers` table, then `FINAL`.

A child sees its chunk as `ctx(text)`, has a slice of the budget, and
returns `answer` (its FINAL's first column) and `detail` (the FINAL row as
JSON). Depth is capped; at the cap, answer in place.

Earlier turns of this session are folded into `turns(n, sql, result)`;
query it instead of re-running a statement whose result scrolled away.
