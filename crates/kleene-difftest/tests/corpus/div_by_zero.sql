CREATE TABLE d (a BIGINT, b DOUBLE);
INSERT INTO d VALUES (1, 1.5), (0, 0.0), (-2, -3.0), (NULL, NULL);
-- query
SELECT a, a / 0 AS i0, b / 0.0 AS f0, a / 0.0 AS if0, 0 / a AS zi, b / 0 AS fi FROM d ORDER BY a NULLS FIRST;
