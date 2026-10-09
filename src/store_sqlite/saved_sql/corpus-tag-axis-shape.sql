-- name: corpus-tag-axis-shape
-- params: 
WITH f AS (SELECT value AS tag, count(*) AS frequency FROM doc_tags GROUP BY value)
SELECT count(tag) AS distinct_tag_count, coalesce(sum(frequency), 0) AS total_tag_mentions, max(frequency) AS max_tag_frequency,
  CASE WHEN count(tag) <= 5 THEN 'tag-axis-empty' WHEN count(tag) <= 30 THEN 'tag-axis-thin' ELSE 'tag-axis-rich' END AS tag_axis_shape
FROM f;
