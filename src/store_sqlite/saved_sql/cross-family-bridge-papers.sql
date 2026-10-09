-- name: cross-family-bridge-papers
-- params: family_a, family_b
SELECT d.id AS id, d.title AS title FROM Doc d
WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = $family_a) AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = $family_b) AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') ORDER BY d.id;
