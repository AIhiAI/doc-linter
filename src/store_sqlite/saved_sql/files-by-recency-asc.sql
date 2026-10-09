-- name: files-by-recency-asc
-- params: 
SELECT f.path AS path, f.last_touched AS last_touched, f.loc AS loc FROM File f WHERE f.last_touched IS NOT NULL ORDER BY f.last_touched ASC, f.path ASC LIMIT 10;
