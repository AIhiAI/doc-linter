-- name: files-by-path-prefix-impl-count-desc
-- params: path_prefix
SELECT f.path AS file_path, count(t.symbol) AS impl_count FROM File f
LEFT JOIN Function t ON t.file = f.path AND NOT (instr(t.symbol, 'tests/') > 0)
WHERE substr(f.path, 1, length($path_prefix)) = $path_prefix AND NOT (instr(f.path, 'tests/') > 0)
GROUP BY f.path ORDER BY impl_count DESC, file_path LIMIT 30;
