-- name: hub-functions
-- params: 
SELECT f.symbol AS "f.symbol", f.file AS "f.file", f.line AS "f.line", m.mentions AS mentions
FROM (SELECT src, count(DISTINCT dst) AS mentions FROM "FUNCTION_MENTIONS" GROUP BY src
      HAVING count(DISTINCT dst) >= 3) m
JOIN Function f ON f.symbol = m.src
ORDER BY mentions DESC, f.symbol LIMIT 50;
