-- name: phasing
-- params: 
SELECT d.phase AS phase, json_group_array(json_object('id', d.id, 'title', d.title)) AS docs, count(d.id) AS count FROM Doc d
GROUP BY d.phase ORDER BY phase;
