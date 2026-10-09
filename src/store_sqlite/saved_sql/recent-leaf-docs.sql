-- name: recent-leaf-docs
-- params: 
SELECT d.id AS id, d.title AS title, d.kind AS kind, d.updated AS updated FROM Doc d
WHERE NOT EXISTS (SELECT 1 FROM "WIKILINK" x WHERE x.dst = d.id) AND d.role = 'doc' ORDER BY d.updated DESC, d.id LIMIT 15;
