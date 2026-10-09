-- name: recent-research-by-update
-- params: 
SELECT d.id AS id, d.title AS title, d.updated AS updated, (SELECT json_group_array(value) FROM doc_tags WHERE doc_id = d.id) AS tags FROM Doc d
WHERE substr(d.id, 1, length('research-')) = 'research-' AND d.updated IS NOT NULL ORDER BY d.updated DESC, d.id LIMIT 10;
