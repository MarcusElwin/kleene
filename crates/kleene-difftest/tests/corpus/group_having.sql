CREATE TABLE edges (src BIGINT, dst BIGINT, w DOUBLE);
INSERT INTO edges VALUES (1, 2, 1.0), (2, 3, 2.5), (3, 1, 0.5), (3, 4, 4.0), (5, 6, NULL);
-- query
SELECT src, COUNT(*) AS n, SUM(w) AS total, AVG(w) AS mean, MAX(dst) AS far
FROM edges GROUP BY src HAVING COUNT(*) >= 1 ORDER BY src;
