-- name: imports-edge-count-by-language
-- params: 
SELECT a.language AS language, count(*) AS imports_count
FROM "IMPORTS" r JOIN File a ON a.path = r.src JOIN File b ON b.path = r.dst
GROUP BY a.language ORDER BY imports_count DESC, language;
