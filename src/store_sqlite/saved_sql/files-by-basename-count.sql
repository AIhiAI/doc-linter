-- name: files-by-basename-count
-- params: basename
SELECT count(f.path) AS file_count FROM File f WHERE substr(f.path, length(f.path) - length($basename) + 1) = $basename AND NOT (instr(f.path, 'tests/') > 0);
