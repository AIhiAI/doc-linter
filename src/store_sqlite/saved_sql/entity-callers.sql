-- name: entity-callers
-- params: entity
SELECT DISTINCT caller.symbol AS "caller.symbol", caller.file AS "caller.file", caller.line AS "caller.line"
FROM Entity e
JOIN (SELECT src, dst FROM "FUNCTION_BELONGS_TO" UNION ALL SELECT src, dst FROM "FUNCTION_MENTIONS") r ON r.dst = e.id
JOIN Function target ON target.symbol = r.src
JOIN "CALLS" c ON c.dst = target.symbol
JOIN Function caller ON caller.symbol = c.src
WHERE e.id = $entity
ORDER BY caller.file, caller.line;
