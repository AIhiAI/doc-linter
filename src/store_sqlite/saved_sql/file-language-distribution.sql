-- name: file-language-distribution
-- params: 
SELECT f.language AS language, count(f.path) AS file_count FROM File f WHERE NOT (instr(f.path, 'tests/') > 0) GROUP BY f.language ORDER BY file_count DESC, language LIMIT 25;
