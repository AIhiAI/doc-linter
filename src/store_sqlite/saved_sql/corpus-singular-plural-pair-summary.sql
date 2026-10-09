-- name: corpus-singular-plural-pair-summary
-- params: 
WITH p AS (SELECT count(*) AS pair_count FROM Entity e1, Entity e2
           WHERE substr(e2.id, 1, length(e1.id)) = e1.id AND substr(e2.id, -1) = 's' AND e1.id <> e2.id)
SELECT pair_count, pair_count * 2 AS total_entities_in_pairs,
  CASE WHEN pair_count = 0 THEN 'no-pairs' WHEN pair_count <= 3 THEN 'sparse-ambiguity' ELSE 'rich-ambiguity' END AS singular_plural_ambiguity_label FROM p;
