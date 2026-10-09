-- name: path-axis-fingerprint-cython-aware
-- params: pattern
WITH c AS (SELECT (SELECT count(DISTINCT f.path) FROM File f WHERE instr(f.path, $pattern) > 0) AS file_axis_count,
  (SELECT coalesce(sum(CASE WHEN (substr(f.path, -4) = '.pyx' OR substr(f.path, -4) = '.pxd') THEN 1 ELSE 0 END), 0) FROM File f WHERE instr(f.path, $pattern) > 0) AS cython_file_count,
  (SELECT count(DISTINCT fn.file) FROM Function fn WHERE instr(fn.file, $pattern) > 0) AS fn_axis_distinct_files,
  (SELECT count(*) FROM Function fn WHERE instr(fn.file, $pattern) > 0) AS fn_total_count)
SELECT file_axis_count, fn_axis_distinct_files, fn_total_count, cython_file_count,
  CASE WHEN cython_file_count >= 1 THEN 'cython-implementation-detected' ELSE CASE WHEN file_axis_count >= 4 AND fn_axis_distinct_files >= 4 THEN 'both-rich' WHEN file_axis_count >= 4 AND fn_axis_distinct_files < 4 THEN 'file-rich-fn-sparse' WHEN file_axis_count < 4 AND fn_axis_distinct_files >= 4 THEN 'file-sparse-fn-rich' ELSE 'both-sparse' END END AS cython_aware_label FROM c;
