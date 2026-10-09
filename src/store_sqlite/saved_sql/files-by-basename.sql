-- name: files-by-basename
-- params: basename
SELECT f.path AS file_path, count(t.symbol) AS impl_count FROM File f
LEFT JOIN Function t ON t.file = f.path AND NOT (instr(t.symbol, 'tests/') > 0)
WHERE substr(f.path, length(f.path) - length($basename) + 1) = $basename AND NOT (instr(f.path, 'tests/') > 0)
GROUP BY f.path ORDER BY file_path LIMIT 50;
