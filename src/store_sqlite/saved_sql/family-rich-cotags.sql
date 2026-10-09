-- name: family-rich-cotags
-- params: family_tag
SELECT t.value AS co_tag, count(DISTINCT d.id) AS member_count, 'rich' AS coverage_label
FROM Doc d JOIN doc_tags t ON t.doc_id = d.id
WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = $family_tag) AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND t.value <> $family_tag AND t.value NOT IN ('research', 'paper', 'production', 'tool', 'doc') AND length(t.value) >= 6 AND NOT (instr(t.value, 'storyline') > 0)
GROUP BY t.value HAVING count(DISTINCT d.id) >= 3 ORDER BY member_count DESC, co_tag;
