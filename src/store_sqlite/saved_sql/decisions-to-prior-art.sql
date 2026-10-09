-- name: decisions-to-prior-art
-- params: 
WITH ed AS (SELECT src, dst, line, 'INFORMED_BY' AS edge_type FROM "INFORMED_BY" UNION ALL SELECT src, dst, line, 'WIKILINK' FROM "WIKILINK")
SELECT s.id AS decision, p.id AS prior_art, r.edge_type AS edge_type, r.line AS line
FROM ed r JOIN Doc s ON s.id = r.src JOIN Doc p ON p.id = r.dst
WHERE (substr(s.path, 1, length('docs/design/')) = 'docs/design/' OR EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = s.id AND value = 'design')) AND (instr(p.id, 'prior-art') > 0 OR p.kind = 'reference')
ORDER BY decision, prior_art, line;
