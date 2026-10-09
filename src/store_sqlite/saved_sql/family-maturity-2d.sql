-- name: family-maturity-2d
-- params: 
WITH rd AS (SELECT d.id FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND NOT EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'index')),
fam AS (SELECT dt.value AS family_tag, count(DISTINCT dt.doc_id) AS member_count
        FROM doc_tags dt JOIN rd ON rd.id = dt.doc_id
        WHERE dt.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system', 'ingest-pass-storyline') AND length(dt.value) >= 6
        GROUP BY dt.value HAVING count(DISTINCT dt.doc_id) >= 3),
sib AS (
  SELECT f.family_tag, f.member_count, s.value AS sibling
  FROM fam f JOIN doc_tags m ON m.value = f.family_tag JOIN rd ON rd.id = m.doc_id
  JOIN doc_tags s ON s.doc_id = m.doc_id
  WHERE s.value <> f.family_tag AND s.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system', 'ingest-pass-storyline') AND NOT (substr(s.value, 1, 5) = 'iter-')),
dep AS (SELECT family_tag, member_count, count(DISTINCT sibling) AS sibling_tag_count,
               member_count * count(DISTINCT sibling) AS depth_score FROM sib GROUP BY family_tag, member_count),
aud AS (SELECT dep.family_tag, dep.member_count, dep.sibling_tag_count, dep.depth_score, count(DISTINCT audit.id) AS audit_count
        FROM dep JOIN Doc audit ON ((substr(audit.id, 1, length('interrogation-')) = 'interrogation-' OR substr(audit.id, 1, length('user-probe-')) = 'user-probe-') AND (instr(audit.title, dep.family_tag) > 0 OR instr(audit.summary, dep.family_tag) > 0))
        GROUP BY dep.family_tag, dep.member_count, dep.sibling_tag_count, dep.depth_score)
SELECT family_tag, member_count, sibling_tag_count, depth_score, audit_count,
  CASE WHEN depth_score >= 100 AND audit_count >= 7 THEN 'deeply-explored-mature'
       WHEN depth_score >= 100 AND audit_count < 7 THEN 'recently-absorbed-umbrella'
       WHEN depth_score < 100 AND audit_count >= 7 THEN 'narrow-but-explored'
       ELSE 'narrow-recent' END AS maturity_label
FROM aud ORDER BY depth_score DESC, audit_count DESC, family_tag LIMIT 25;
