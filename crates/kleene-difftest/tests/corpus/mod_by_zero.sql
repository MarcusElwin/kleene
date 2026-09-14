CREATE TABLE d (a BIGINT, b DOUBLE);
INSERT INTO d VALUES (1, 1.5), (0, 0.0), (-2, -3.0), (NULL, NULL);
-- query
SELECT a, a % 0 AS m, b % 0.0 AS mf, a % 2 AS m2, b % 2 AS mf2 FROM d ORDER BY a NULLS FIRST;
