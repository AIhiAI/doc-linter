-- name: family-tag-audit-counts-summary
-- params: 
WITH rd AS (SELECT d.id FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND NOT EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'index')),
fam AS (SELECT dt.value AS family_tag, count(DISTINCT dt.doc_id) AS member_count
        FROM doc_tags dt JOIN rd ON rd.id = dt.doc_id
        WHERE dt.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system', 'ingest-pass-storyline') AND length(dt.value) >= 6
        GROUP BY dt.value HAVING count(DISTINCT dt.doc_id) >= 3)
SELECT f.family_tag AS family_tag, f.member_count AS member_count, count(DISTINCT audit.id) AS audit_count
FROM fam f JOIN Doc audit ON ((substr(audit.id, 1, length('interrogation-')) = 'interrogation-' OR substr(audit.id, 1, length('user-probe-')) = 'user-probe-') AND (instr(audit.title, f.family_tag) > 0 OR instr(audit.summary, f.family_tag) > 0))
GROUP BY f.family_tag, f.member_count ORDER BY audit_count DESC, family_tag LIMIT 25;
