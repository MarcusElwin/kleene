CREATE TABLE nodes (id BIGINT, name VARCHAR);
INSERT INTO nodes VALUES (1, 'a'), (2, 'b'), (3, 'c'), (4, NULL), (7, 'g');
-- query
SELECT id, CASE WHEN id > 3 THEN 'big' ELSE 'small' END AS size, CAST(id AS VARCHAR) || '!' AS tag,
       name LIKE '_' AS single, coalesce(name, 'none') AS nm, id / 2 AS half
FROM nodes ORDER BY id;
