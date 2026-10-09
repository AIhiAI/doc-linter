-- name: entity-singular-plural-pairs
-- params: 
SELECT e1.id AS singular_id, e1.mention_count AS singular_mention_count, e2.id AS plural_id, e2.mention_count AS plural_mention_count,
       e1.mention_count + e2.mention_count AS combined_mention_count
FROM Entity e1, Entity e2
WHERE substr(e2.id, 1, length(e1.id)) = e1.id AND substr(e2.id, -1) = 's' AND e1.id <> e2.id
ORDER BY combined_mention_count DESC, e1.id LIMIT 20;
