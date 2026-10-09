-- name: corpus-update-span
-- params: 
SELECT min(d.updated) AS oldest_doc, max(d.updated) AS newest_doc, count(d.id) AS doc_count FROM Doc d WHERE d.updated IS NOT NULL;
