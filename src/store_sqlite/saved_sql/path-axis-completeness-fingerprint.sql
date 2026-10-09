-- name: path-axis-completeness-fingerprint
-- params: pattern
WITH c AS (SELECT (SELECT count(DISTINCT f.path) FROM File f WHERE instr(f.path, $pattern) > 0) AS file_axis_count,
  (SELECT count(DISTINCT fn.file) FROM Function fn WHERE instr(fn.file, $pattern) > 0) AS fn_axis_distinct_files,
  (SELECT count(*) FROM Function fn WHERE instr(fn.file, $pattern) > 0) AS fn_total_count)
SELECT file_axis_count, fn_axis_distinct_files, fn_total_count, CASE WHEN file_axis_count >= 4 AND fn_axis_distinct_files >= 4 THEN 'both-rich' WHEN file_axis_count >= 4 AND fn_axis_distinct_files < 4 THEN 'file-rich-fn-sparse' WHEN file_axis_count < 4 AND fn_axis_distinct_files >= 4 THEN 'file-sparse-fn-rich' ELSE 'both-sparse' END AS axis_completeness_label FROM c;
