CREATE TABLE nodes (id BIGINT, name VARCHAR);
INSERT INTO nodes VALUES (1, 'a'), (2, 'b'), (3, 'c'), (4, NULL), (7, 'g');
-- query
SELECT n.id, g.generate_series AS k
FROM nodes n CROSS JOIN LATERAL generate_series(1, n.id) AS g
WHERE n.id < 4 ORDER BY 1, 2;
