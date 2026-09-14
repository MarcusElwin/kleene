CREATE TABLE edges (src BIGINT, dst BIGINT, w DOUBLE);
INSERT INTO edges VALUES (1, 2, 1.0), (2, 3, 2.5), (3, 1, 0.5), (3, 4, 4.0), (5, 6, NULL);
CREATE TABLE nodes (id BIGINT, name VARCHAR);
INSERT INTO nodes VALUES (1, 'a'), (2, 'b'), (3, 'c'), (4, NULL), (7, 'g');
-- query
WITH RECURSIVE reach(n) AS (
  SELECT dst FROM edges WHERE src = 1
  UNION
  SELECT e.dst FROM reach r JOIN edges e ON e.src = r.n
)
SELECT n FROM reach ORDER BY n;
