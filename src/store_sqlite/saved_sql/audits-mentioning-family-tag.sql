-- name: audits-mentioning-family-tag
-- params: tag
SELECT audit.id AS audit_id, audit.title AS audit_title, audit.updated AS audit_updated FROM Doc audit
WHERE ((substr(audit.id, 1, length('interrogation-')) = 'interrogation-' OR substr(audit.id, 1, length('user-probe-')) = 'user-probe-') AND (instr(audit.title, $tag) > 0 OR instr(audit.summary, $tag) > 0))
ORDER BY audit.updated DESC, audit.id LIMIT 25;
