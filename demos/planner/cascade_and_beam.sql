-- Cascades and beam-limited recursion. Run with:
--   kleene repl < demos/planner/cascade_and_beam.sql

CREATE TABLE papers AS
  SELECT generate_series AS id,
         (CASE generate_series % 4 WHEN 0 THEN 'graph' WHEN 1 THEN 'tree' WHEN 2 THEN 'sort' ELSE 'hash' END)
           || ' paper ' || generate_series AS title
  FROM generate_series(0, 99);

-- A cheap scorer on the proxy tier and the expensive oracle that declares it.
CREATE FUNCTION relevance_score(title TEXT) RETURNS DOUBLE AS PROMPT 'Score from 0 to 1 how likely ''{title}'' is about graph algorithms. Answer with the number only.' MODEL 'proxy';
CREATE FUNCTION relevant(title TEXT) RETURNS BOOLEAN AS PROMPT 'Is the paper ''{title}'' about graph algorithms? Answer yes or no.'
  PROXY relevance_score THRESHOLDS (0.2, 0.8);

-- The filter becomes  score >= 0.8 OR (score >= 0.2 AND relevant(title)):
-- 100 proxy calls at a tenth of the price, and the oracle only for the band.
EXPLAIN SELECT title FROM papers WHERE relevant(title);

-- Beam search: the trailing ORDER BY ... LIMIT keeps the best five new
-- hypotheses of each round instead of letting the frontier grow 4x per round
-- (expand(h, n) is the built-in table call that asks the model for n items).
SET max_recursion_rounds = 3;
EXPLAIN WITH RECURSIVE f(h, d) AS (
  SELECT title, 0 FROM papers WHERE id < 10
  UNION ALL
  SELECT e.item, f.d + 1 FROM f CROSS JOIN LATERAL expand(f.h, 4) AS e WHERE f.d < 3
  ORDER BY e.item LIMIT 5
) SELECT h FROM f;
