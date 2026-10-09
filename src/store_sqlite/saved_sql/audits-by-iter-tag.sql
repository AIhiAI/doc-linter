-- name: audits-by-iter-tag
-- params: iter_tag
SELECT audit.id AS audit_id, audit.title AS audit_title, (SELECT json_group_array(value) FROM doc_tags WHERE doc_id = audit.id) AS audit_tags, audit.updated AS audit_updated
FROM Doc audit
WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = audit.id AND value = $iter_tag) AND (substr(audit.id, 1, length('interrogation-')) = 'interrogation-' OR substr(audit.id, 1, length('user-probe-')) = 'user-probe-')
ORDER BY audit.id LIMIT 25;
