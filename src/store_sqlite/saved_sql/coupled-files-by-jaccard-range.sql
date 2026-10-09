-- name: coupled-files-by-jaccard-range
-- params: min_jaccard, max_jaccard
SELECT a.path AS "a.path", b.path AS "b.path", r.commits AS "r.commits", r.jaccard AS "r.jaccard", r.last_co_change_at AS "r.last_co_change_at"
FROM "COUPLED_WITH" r JOIN File a ON a.path = r.src JOIN File b ON b.path = r.dst WHERE r.jaccard >= $min_jaccard AND r.jaccard <= $max_jaccard ORDER BY r.jaccard DESC, r.commits DESC LIMIT 50;
