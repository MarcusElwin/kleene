CREATE TABLE edges (src BIGINT, dst BIGINT, w DOUBLE);
INSERT INTO edges VALUES (1, 2, 1.0), (2, 3, 2.5), (3, 1, 0.5), (3, 4, 4.0), (5, 6, NULL);
-- query
WITH RECURSIVE p(n, d) AS (
  SELECT 1, 0
  UNION ALL
  SELECT e.dst, p.d + 1 FROM p JOIN edges e ON e.src = p.n WHERE p.d < 4
)
SELECT n, MIN(d) AS hops FROM p GROUP BY n ORDER BY n;
