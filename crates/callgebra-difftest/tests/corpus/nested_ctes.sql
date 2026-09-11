CREATE TABLE edges (src BIGINT, dst BIGINT, w DOUBLE);
INSERT INTO edges VALUES (1, 2, 1.0), (2, 3, 2.5), (3, 1, 0.5), (3, 4, 4.0), (5, 6, NULL);
-- query
WITH heavy AS (SELECT src, dst FROM edges WHERE w > 1),
     out_deg AS (SELECT src, COUNT(*) AS n FROM heavy GROUP BY src)
SELECT h.src, h.dst, o.n FROM heavy h JOIN out_deg o ON o.src = h.src ORDER BY 1, 2;
