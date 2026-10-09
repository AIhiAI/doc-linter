-- name: roles-using-tag
-- params: tag
SELECT d.role AS role, count(d.id) AS doc_count FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = $tag) GROUP BY d.role ORDER BY doc_count DESC, role;
