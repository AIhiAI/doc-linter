-- name: function-count-by-language
-- params: 
SELECT f.language AS language, count(f.symbol) AS count FROM Function f GROUP BY f.language ORDER BY count DESC, language;
