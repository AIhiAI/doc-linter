-- name: untested-functions
-- params: 
SELECT f.symbol AS "f.symbol", f.file AS "f.file", f.line AS "f.line", f.language AS "f.language" FROM Function f
WHERE NOT EXISTS (SELECT 1 FROM "TEST_FOR" t WHERE t.dst = f.symbol) AND NOT (instr(f.file, 'test') > 0)
ORDER BY f.file, f.line;
