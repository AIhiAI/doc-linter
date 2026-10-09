-- name: file-language-by-path
-- params: file_path
SELECT f.path AS file_path, f.language AS language FROM File f WHERE f.path = $file_path;
