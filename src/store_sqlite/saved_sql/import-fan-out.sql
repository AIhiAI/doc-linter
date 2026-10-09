-- name: import-fan-out
-- params: 
SELECT imported.path AS "imported.path", imported.language AS "imported.language", count(DISTINCT importer.path) AS importers
FROM "IMPORTS" r JOIN File importer ON importer.path = r.src JOIN File imported ON imported.path = r.dst
GROUP BY imported.path ORDER BY importers DESC, imported.path LIMIT 50;
