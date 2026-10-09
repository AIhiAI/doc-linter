-- name: docs-by-summary-substring
-- params: substring
SELECT d.id AS doc_id, d.title AS title, d.summary AS summary FROM Doc d WHERE d.summary IS NOT NULL AND instr(d.summary, $substring) > 0 ORDER BY d.id LIMIT 30;
