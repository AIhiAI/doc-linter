-- name: lessons-for-research-family
-- params: tag
WITH fm AS (SELECT d.id FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = $tag))
SELECT audit.id AS audit_id, audit.title AS audit_title, count(DISTINCT w.dst) AS cites_count,
       json_group_array(DISTINCT w.dst) AS cites_in_family, audit.updated AS audit_updated
FROM "WIKILINK" w JOIN Doc audit ON audit.id = w.src JOIN fm ON fm.id = w.dst
WHERE (substr(audit.id, 1, length('interrogation-')) = 'interrogation-' OR substr(audit.id, 1, length('user-probe-')) = 'user-probe-')
GROUP BY audit.id ORDER BY cites_count DESC, audit_updated DESC, audit.id LIMIT 25;
