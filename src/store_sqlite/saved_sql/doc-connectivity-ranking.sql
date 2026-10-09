-- name: doc-connectivity-ranking
-- params: 
SELECT d.id AS id, d.title AS title, d.role AS role, count(*) AS total_connections
FROM Doc d JOIN (SELECT src, dst FROM "WIKILINK" UNION ALL SELECT dst, src FROM "WIKILINK") l ON l.src = d.id JOIN Doc o ON o.id = l.dst
GROUP BY d.id HAVING count(*) >= 3 ORDER BY total_connections DESC, d.id LIMIT 25;
