-- name: files-by-basename-and-path-prefix-count
-- params: basename, path_prefix
SELECT count(f.path) AS file_count FROM File f WHERE substr(f.path, length(f.path) - length($basename) + 1) = $basename AND substr(f.path, 1, length($path_prefix)) = $path_prefix AND NOT (instr(f.path, 'tests/') > 0);
