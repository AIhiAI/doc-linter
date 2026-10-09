-- name: callgraph-roots
-- params: 
SELECT f.symbol AS "f.symbol", f.file AS "f.file", f.line AS "f.line", f.language AS "f.language" FROM Function f
WHERE NOT EXISTS (SELECT 1 FROM "CALLS" c WHERE c.dst = f.symbol) ORDER BY f.file, f.line;
