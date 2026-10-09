-- name: family-to-feature-bridges
-- params: tag
SELECT r.id AS research_id, r.title AS research_title, f.id AS feature_id, f.title AS feature_title
FROM Doc r JOIN "WIKILINK" w ON w.src = r.id JOIN Doc f ON f.id = w.dst
WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = r.id AND value = 'research') AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = r.id AND value = $tag) AND f.role = 'feature'
ORDER BY research_id, feature_id LIMIT 25;
