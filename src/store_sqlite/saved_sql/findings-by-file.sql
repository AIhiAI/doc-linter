-- name: findings-by-file
-- params: 
SELECT file.path AS "file.path", count(fnd.id) AS findings, json_group_array(DISTINCT fnd.kind) AS sample_kinds
FROM File file JOIN "HAS_FINDING" h ON h.src = file.path JOIN Finding fnd ON fnd.id = h.dst
GROUP BY file.path ORDER BY findings DESC, file.path;
