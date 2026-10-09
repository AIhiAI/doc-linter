-- name: findings-by-attachment-mode
-- params: 
SELECT attachment_mode, count(*) AS finding_count FROM (
  SELECT CASE WHEN f.kind = 'unstubbed-concept' THEN 'doc-unstubbed-concept' ELSE 'source-code-marker' END AS attachment_mode FROM Finding f)
GROUP BY attachment_mode ORDER BY finding_count DESC;
