-- name: cross-family-bridge-density
-- params: 
SELECT a.value AS family_a, b.value AS family_b, count(DISTINCT d.id) AS bridge_count
FROM Doc d JOIN doc_tags a ON a.doc_id = d.id JOIN doc_tags b ON b.doc_id = d.id
WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND a.value NOT IN ('research', 'paper', 'production', 'tool', 'doc') AND length(a.value) >= 6 AND NOT (instr(a.value, 'storyline') > 0) AND b.value NOT IN ('research', 'paper', 'production', 'tool', 'doc') AND length(b.value) >= 6 AND NOT (instr(b.value, 'storyline') > 0) AND b.value > a.value
GROUP BY a.value, b.value HAVING count(DISTINCT d.id) >= 3
ORDER BY bridge_count DESC, family_a, family_b LIMIT 25;
