-- name: research-pairs-without-shared-family-tag
-- params: 
SELECT a.id AS doc_a, b.id AS doc_b, a.title AS title_a, b.title AS title_b
FROM "WIKILINK" w JOIN Doc a ON a.id = w.src JOIN Doc b ON b.id = w.dst
WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = a.id AND value = 'research') AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = b.id AND value = 'research') AND NOT EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = a.id AND value = 'index') AND NOT EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = b.id AND value = 'index')
  AND a.id < b.id
  AND 0 = (SELECT count(*) FROM doc_tags xa JOIN doc_tags xb ON xb.doc_id = b.id AND xb.value = xa.value
           WHERE xa.doc_id = a.id AND xa.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system'))
ORDER BY a.id, b.id;
