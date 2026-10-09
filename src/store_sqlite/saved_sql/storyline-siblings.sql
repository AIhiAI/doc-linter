-- name: storyline-siblings
-- params: doc_id
SELECT o.id AS doc_id, o.title AS title, count(*) AS shared_tag_count
FROM Doc o JOIN doc_tags ot ON ot.doc_id = o.id JOIN doc_tags st ON st.value = ot.value AND st.doc_id = $doc_id
WHERE o.id <> $doc_id AND o.role = 'doc' AND st.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system', 'substantial-bundle', 'user-probe', 'interrogation') AND EXISTS (SELECT 1 FROM Doc WHERE id = $doc_id)
GROUP BY o.id ORDER BY shared_tag_count DESC, o.id LIMIT 25;
