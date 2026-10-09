-- name: family-tag-absorption-depth
-- params: 
WITH rd AS (SELECT d.id FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND NOT EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'index')),
fam AS (SELECT dt.value AS family_tag, count(DISTINCT dt.doc_id) AS member_count
        FROM doc_tags dt JOIN rd ON rd.id = dt.doc_id
        WHERE dt.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system', 'ingest-pass-storyline')
        GROUP BY dt.value HAVING count(DISTINCT dt.doc_id) >= 3),
sib AS (
  SELECT f.family_tag, f.member_count, s.value AS sibling
  FROM fam f JOIN doc_tags m ON m.value = f.family_tag JOIN rd ON rd.id = m.doc_id
  JOIN doc_tags s ON s.doc_id = m.doc_id
  WHERE s.value <> f.family_tag AND s.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system', 'ingest-pass-storyline') AND NOT (substr(s.value, 1, 5) = 'iter-'))
SELECT family_tag, member_count, count(DISTINCT sibling) AS sibling_tag_count,
       member_count * count(DISTINCT sibling) AS depth_score
FROM sib GROUP BY family_tag, member_count ORDER BY depth_score DESC, family_tag LIMIT 25;
