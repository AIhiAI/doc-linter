-- name: corpus-axis-saturation
-- params: 
WITH c AS (SELECT
  (SELECT count(*) FROM Doc) AS doc_count,
  (SELECT count(*) FROM Doc WHERE NOT (summary IS NULL OR summary = '')) AS docs_with_summary,
  (SELECT count(*) FROM File) AS file_count,
  (SELECT count(*) FROM File WHERE NOT (language IS NULL OR language = '')) AS files_with_language,
  (SELECT count(*) FROM Function) AS function_count,
  (SELECT count(*) FROM Function WHERE NOT (signature IS NULL OR signature = '')) AS functions_with_signature)
SELECT doc_count, docs_with_summary, file_count, files_with_language, function_count, functions_with_signature,
  CASE WHEN doc_count = 0 THEN 0.0 ELSE 1.0 * docs_with_summary / doc_count END AS docs_summary_fraction,
  CASE WHEN file_count = 0 THEN 0.0 ELSE 1.0 * files_with_language / file_count END AS files_language_fraction,
  CASE WHEN function_count = 0 THEN 0.0 ELSE 1.0 * functions_with_signature / function_count END AS functions_signature_fraction
FROM c;
