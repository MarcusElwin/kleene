CREATE TABLE edges (src BIGINT, dst BIGINT, w DOUBLE);
INSERT INTO edges VALUES (1, 2, 1.0), (2, 3, 2.5), (3, 1, 0.5), (3, 4, 4.0), (5, 6, NULL);
CREATE TABLE nodes (id BIGINT, name VARCHAR);
INSERT INTO nodes VALUES (1, 'a'), (2, 'b'), (3, 'c'), (4, NULL), (7, 'g');
-- query
SELECT n.id, n.name FROM nodes n
WHERE NOT EXISTS (SELECT 1 FROM edges e WHERE e.src = n.id)
ORDER BY n.id;
