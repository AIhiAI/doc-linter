-- name: feature-incoming-from-research
-- params: 
SELECT f.id AS feature_id, f.title AS feature_title,
  (SELECT count(*) FROM "WIKILINK" w JOIN Doc r ON r.id = w.src WHERE w.dst = f.id AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = r.id AND value = 'research')) AS research_incoming_count
FROM Doc f WHERE f.role = 'feature' ORDER BY research_incoming_count DESC, f.id LIMIT 25;
