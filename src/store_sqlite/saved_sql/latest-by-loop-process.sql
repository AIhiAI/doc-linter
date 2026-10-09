-- name: latest-by-loop-process
-- params: 
WITH pc AS (SELECT d.id AS id, d.updated AS updated, CASE WHEN substr(d.id, 1, length('research-')) = 'research-' THEN 'research' WHEN substr(d.id, 1, length('interrogation-')) = 'interrogation-' THEN 'interrogation' WHEN substr(d.id, 1, length('user-probe-')) = 'user-probe-' THEN 'user-probe' WHEN substr(d.id, 1, length('audit-run-')) = 'audit-run-' THEN 'audit-run' ELSE 'other' END AS process_class FROM Doc d),
mx AS (SELECT process_class, max(updated) AS max_updated FROM pc
       WHERE process_class <> 'other' AND updated IS NOT NULL AND updated <> '' GROUP BY process_class),
lt AS (SELECT mx.process_class AS process_class, max(pc.id) AS latest_id FROM mx
       JOIN pc ON pc.updated = mx.max_updated AND pc.process_class = mx.process_class GROUP BY mx.process_class)
SELECT lt.process_class AS process_class, f.id AS latest_doc_id, f.title AS latest_title, f.updated AS latest_updated
FROM lt JOIN Doc f ON f.id = lt.latest_id ORDER BY process_class;
