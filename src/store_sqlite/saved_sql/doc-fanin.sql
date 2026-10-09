-- name: doc-fanin
-- params: 
SELECT d.id AS id, d.title AS title, d.kind AS kind, d.role AS role, d.status AS status, count(DISTINCT w.src) AS in_degree
FROM Doc d JOIN "WIKILINK" w ON w.dst = d.id JOIN Doc s ON s.id = w.src
GROUP BY d.id ORDER BY in_degree DESC, d.id ASC LIMIT 20;
