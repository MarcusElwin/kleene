-- Three relations, two LLM predicates. Run with:
--   kleene repl < demos/planner/three_way.sql
-- The tables are synthetic; the predicates are prompt functions, so a
-- provider is needed only for the final SELECT (the EXPLAINs are free).

CREATE TABLE papers AS
  SELECT generate_series AS id,
         (CASE generate_series % 4 WHEN 0 THEN 'graph' WHEN 1 THEN 'tree' WHEN 2 THEN 'sort' ELSE 'hash' END)
           || ' paper ' || generate_series AS title
  FROM generate_series(0, 19);
CREATE TABLE claims AS
  SELECT generate_series AS id, (generate_series * 7) % 20 AS paper_id,
         'claim ' || generate_series AS text
  FROM generate_series(0, 19);
CREATE TABLE evidence AS
  SELECT (generate_series * 3) % 20 AS claim_id, 'snippet ' || generate_series AS snippet
  FROM generate_series(0, 19);

CREATE FUNCTION relevant(title TEXT) RETURNS BOOLEAN AS PROMPT 'Is the paper ''{title}'' about graph algorithms? Answer yes or no.';
CREATE FUNCTION supports(claim TEXT, snippet TEXT) RETURNS BOOLEAN AS PROMPT 'Does ''{snippet}'' support ''{claim}''? Answer yes or no.';

-- As written this is a cross product with the predicates on top: the naive
-- plan would ask `supports` for every one of the 8,000 (paper, claim,
-- evidence) triples. EXPLAIN shows the plan space (3 relations, 12 bushy
-- orders), the plan chosen and the as-written cost beside it.
EXPLAIN SELECT p.title, c.text, e.snippet
FROM papers p, claims c, evidence e
WHERE supports(c.text, e.snippet) AND relevant(p.title)
  AND c.paper_id = p.id AND e.claim_id = c.id;

-- Seven relations: 665,280 bushy join orders, priced through 1,932 subset
-- splits at most (dynamic programming with branch-and-bound).
EXPLAIN SELECT p1.title
FROM papers p1, papers p2, papers p3, papers p4, papers p5, papers p6, papers p7
WHERE p1.id = p2.id AND p2.id = p3.id AND p3.id = p4.id AND p4.id = p5.id
  AND p5.id = p6.id AND p6.id = p7.id
  AND relevant(p1.title) AND supports(p3.title, p7.title);
