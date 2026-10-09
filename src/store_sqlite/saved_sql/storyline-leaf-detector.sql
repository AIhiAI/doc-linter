-- name: storyline-leaf-detector
-- params: 
SELECT d.id AS id, d.title AS title, d.role AS role, d.updated AS updated FROM Doc d
WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'ingest-pass-storyline') AND NOT EXISTS (SELECT 1 FROM "WIKILINK" x WHERE x.dst = d.id)
ORDER BY d.updated DESC, d.id;
