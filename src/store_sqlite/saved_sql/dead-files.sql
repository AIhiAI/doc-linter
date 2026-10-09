-- name: dead-files
-- params: 
SELECT f.path AS "f.path", f.language AS "f.language", f.loc AS "f.loc" FROM File f
WHERE EXISTS (SELECT 1 FROM "DEFINED_IN_FILE" d WHERE d.dst = f.path)
  AND NOT EXISTS (SELECT 1 FROM "DEFINED_IN_FILE" d WHERE d.dst = f.path
        AND (EXISTS (SELECT 1 FROM "CALLS" c WHERE c.dst = d.src)
             OR EXISTS (SELECT 1 FROM "ENDPOINT_HANDLED_BY" h WHERE h.dst = d.src)))
ORDER BY f.loc DESC;
