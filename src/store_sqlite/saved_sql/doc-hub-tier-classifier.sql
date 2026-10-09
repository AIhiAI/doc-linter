-- name: doc-hub-tier-classifier
-- params: 
SELECT d.id AS id, d.title AS title, d.role AS role, count(*) AS total_connections,
  CASE WHEN count(*) >= 40 THEN 'tier-1-architectural-narrative' WHEN count(*) >= 25 THEN 'tier-2-substantial'
       WHEN count(*) >= 15 THEN 'tier-3-active-participation' ELSE 'tier-4-leaf-or-feature' END AS tier_label
FROM Doc d JOIN (SELECT src, dst FROM "WIKILINK" UNION ALL SELECT dst, src FROM "WIKILINK") l ON l.src = d.id JOIN Doc o ON o.id = l.dst
GROUP BY d.id HAVING count(*) >= 3 ORDER BY total_connections DESC, d.id LIMIT 25;
