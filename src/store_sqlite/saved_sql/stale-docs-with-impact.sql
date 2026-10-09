-- name: stale-docs-with-impact
-- params: 
SELECT d.id AS id, d.kind AS kind, d.updated AS updated,
  (SELECT count(*) FROM "WIKILINK" w JOIN Doc s ON s.id = w.src WHERE w.dst = d.id) AS inbound_count, d.title AS title
FROM Doc d WHERE d.updated IS NOT NULL AND NOT d.role IN ('ontology-value', 'ontology-axis', 'ontology-entity', 'ontology-migration', 'index') AND d.kind <> ''
ORDER BY d.updated ASC, inbound_count ASC, d.id LIMIT 20;
