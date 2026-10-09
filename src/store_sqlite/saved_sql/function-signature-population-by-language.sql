-- name: function-signature-population-by-language
-- params: 
SELECT language, function_count, populated_signatures,
  CASE WHEN function_count = 0 THEN 0 ELSE 100 * populated_signatures / function_count END AS percent_populated
FROM (SELECT f.language AS language, count(f.symbol) AS function_count,
        sum(CASE WHEN f.signature IS NULL OR f.signature = '' THEN 0 ELSE 1 END) AS populated_signatures
      FROM Function f GROUP BY f.language)
ORDER BY function_count DESC, language;
