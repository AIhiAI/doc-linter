-- name: doc-role-distribution
-- params: 
SELECT d.role AS role, count(d.id) AS doc_count FROM Doc d GROUP BY d.role ORDER BY doc_count DESC, role LIMIT 25;
