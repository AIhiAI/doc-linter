-- name: files-under-folder-path
-- params: folder_path
SELECT f.path AS file_path, f.loc AS loc FROM File f WHERE instr(f.path, $folder_path) > 0 AND NOT (instr(f.path, 'test') > 0) ORDER BY f.loc DESC LIMIT 30;
