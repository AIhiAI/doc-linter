-- name: freshness-by-kind
-- params: 
SELECT d.kind AS kind, count(d.id) AS doc_count, min(d.updated) AS oldest_updated, max(d.updated) AS newest_updated FROM Doc d
WHERE d.updated IS NOT NULL AND NOT d.role IN ('ontology-value', 'ontology-axis', 'ontology-entity', 'ontology-migration', 'index') AND d.kind IS NOT NULL AND d.kind <> ''
GROUP BY d.kind ORDER BY oldest_updated ASC, kind;
