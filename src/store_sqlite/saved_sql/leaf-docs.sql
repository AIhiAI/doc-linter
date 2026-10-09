-- name: leaf-docs
-- params: 
SELECT d.id AS id, d.title AS title, d.kind AS kind, d.role AS role, d.status AS status FROM Doc d WHERE NOT EXISTS (SELECT 1 FROM "WIKILINK" x WHERE x.dst = d.id) AND NOT EXISTS (SELECT 1 FROM "MD_LINK" x WHERE x.dst = d.id) AND NOT EXISTS (SELECT 1 FROM "INFORMED_BY" x WHERE x.dst = d.id) ORDER BY d.id;
