-- name: mature-topic-shape-census
-- params: 
WITH fam AS (
  SELECT t.value AS family_tag, count(DISTINCT d.id) AS total_members
  FROM Doc d JOIN doc_tags t ON t.doc_id = d.id
  WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND t.value NOT IN ('research', 'paper', 'production', 'tool', 'doc') AND length(t.value) >= 6 AND NOT (instr(t.value, 'storyline') > 0)
  GROUP BY t.value HAVING count(DISTINCT d.id) >= 5),
co AS (
  SELECT f.family_tag, f.total_members, c.value AS co_tag, count(DISTINCT d.id) AS co_member_count
  FROM fam f JOIN doc_tags ft ON ft.value = f.family_tag JOIN Doc d ON d.id = ft.doc_id
  JOIN doc_tags c ON c.doc_id = d.id
  WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND c.value <> f.family_tag AND c.value NOT IN ('research', 'paper', 'production', 'tool', 'doc') AND length(c.value) >= 6 AND NOT (instr(c.value, 'storyline') > 0)
  GROUP BY f.family_tag, c.value),
agg AS (
  SELECT family_tag, total_members,
    count(CASE WHEN co_member_count >= 3 THEN co_tag END) AS rich_subfamilies,
    count(CASE WHEN co_member_count = 2 THEN co_tag END) AS couple_subfamilies,
    count(CASE WHEN co_member_count = 1 THEN co_tag END) AS singleton_cotags
  FROM co GROUP BY family_tag, total_members)
SELECT family_tag, total_members, rich_subfamilies, couple_subfamilies, singleton_cotags,
  CASE WHEN rich_subfamilies >= 2 AND singleton_cotags <= 2 THEN 'DEEP-mature'
       WHEN rich_subfamilies = 0 AND couple_subfamilies <= 1 AND singleton_cotags >= 4 THEN 'BROAD-mature'
       WHEN rich_subfamilies >= 1 AND singleton_cotags >= 2 THEN 'MIXED-mature'
       ELSE 'emergent' END AS shape_label
FROM agg ORDER BY total_members DESC, family_tag LIMIT 25;
