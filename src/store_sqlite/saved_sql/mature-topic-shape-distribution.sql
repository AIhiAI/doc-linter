-- name: mature-topic-shape-distribution
-- params: family_tag
SELECT co_tag, member_count,
  CASE WHEN member_count >= 3 THEN 'rich' WHEN member_count = 2 THEN 'couple' WHEN member_count = 1 THEN 'singleton' END AS coverage_label
FROM (SELECT t.value AS co_tag, count(DISTINCT d.id) AS member_count
      FROM Doc d JOIN doc_tags t ON t.doc_id = d.id
      WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = $family_tag) AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND t.value <> $family_tag AND t.value NOT IN ('research', 'paper', 'production', 'tool', 'doc') AND length(t.value) >= 6 AND NOT (instr(t.value, 'storyline') > 0)
      GROUP BY t.value)
ORDER BY member_count DESC, co_tag LIMIT 30;
