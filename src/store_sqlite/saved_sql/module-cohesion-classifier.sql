-- name: module-cohesion-classifier
-- params: pattern
WITH base AS (SELECT t.symbol AS sym, t.file AS file FROM Function t WHERE instr(t.symbol, $pattern) > 0 AND NOT (instr(t.file, 'tests/') > 0) AND NOT (instr(t.symbol, 'tests/') > 0)),
c AS (SELECT (SELECT count(*) FROM base) AS impl_count,
  (SELECT count(*) FROM base b JOIN "CALLS" r ON r.src = b.sym JOIN Function c1 ON c1.symbol = r.dst WHERE c1.file = b.file) AS internal_count,
  (SELECT count(*) FROM base b JOIN "CALLS" r ON r.src = b.sym JOIN Function c2 ON c2.symbol = r.dst WHERE c2.file <> b.file) AS external_count)
SELECT impl_count, internal_count, external_count, CASE WHEN internal_count + external_count = 0 THEN 0.0 ELSE external_count * 100.0 / (internal_count + external_count) END AS external_pct, CASE WHEN impl_count = 0 THEN 'empty' WHEN internal_count + external_count = 0 THEN 'stub-only-cython' WHEN external_count = 0 THEN 'cohesive' WHEN impl_count <= 2 AND internal_count = 0 THEN 'leaf-with-deps' WHEN external_count * 100 / (internal_count + external_count) <= 10 THEN 'mostly-cohesive' WHEN external_count * 100 / (internal_count + external_count) <= 67 THEN 'balanced-dependent' ELSE 'heavily-dependent' END AS cohesion_label FROM c;
