-- name: function-mentions-by-language
-- params: 
SELECT f.language AS language, count(*) AS mention_count FROM Function f
JOIN "FUNCTION_MENTIONS" r ON r.src = f.symbol JOIN Entity e ON e.id = r.dst
GROUP BY f.language ORDER BY mention_count DESC, language;
