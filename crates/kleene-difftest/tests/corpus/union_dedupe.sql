CREATE TABLE edges (src BIGINT, dst BIGINT, w DOUBLE);
INSERT INTO edges VALUES (1, 2, 1.0), (2, 3, 2.5), (3, 1, 0.5), (3, 4, 4.0), (5, 6, NULL);
-- query
SELECT src AS v FROM edges UNION SELECT dst FROM edges ORDER BY 1;
