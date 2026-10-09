-- name: family-top-bridges
-- params: family_tag
SELECT fb.value AS other_family, count(DISTINCT d.id) AS bridge_count
FROM Doc d JOIN doc_tags fb ON fb.doc_id = d.id
WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = $family_tag) AND EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND fb.value <> $family_tag AND fb.value NOT IN ('research', 'paper', 'production', 'tool', 'doc') AND length(fb.value) >= 6 AND NOT (instr(fb.value, 'storyline') > 0)
GROUP BY fb.value HAVING count(DISTINCT d.id) >= 2 ORDER BY bridge_count DESC, other_family LIMIT 20;
