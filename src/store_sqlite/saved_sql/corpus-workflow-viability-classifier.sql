-- name: corpus-workflow-viability-classifier
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM Doc d WHERE d.role = 'doc') AS narrative_doc_count,
  (SELECT count(*) FROM File f WHERE (substr(f.path, -4) = '.pyx' OR substr(f.path, -4) = '.pxd') AND NOT (instr(f.path, 'tests/') > 0)) AS cython_file_count)
SELECT narrative_doc_count, cython_file_count,
  CASE WHEN narrative_doc_count >= 10 THEN 'architecture-viable' WHEN cython_file_count > 0 THEN 'cython-blocked' ELSE 'pure-python-narrative-sparse' END AS viability_label FROM c;
