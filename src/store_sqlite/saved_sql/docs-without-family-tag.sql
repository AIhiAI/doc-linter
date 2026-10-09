-- name: docs-without-family-tag
-- params: 
SELECT d.id AS doc_id, d.title AS title, (SELECT json_group_array(value) FROM doc_tags WHERE doc_id = d.id) AS tags FROM Doc d
WHERE d.role = 'doc' AND NOT (substr(d.id, 1, length('audit-run-')) = 'audit-run-') AND NOT (substr(d.id, 1, length('feature-')) = 'feature-')
  AND NOT EXISTS (SELECT 1 FROM doc_tags dt WHERE dt.doc_id = d.id AND dt.value NOT IN
      ('research','paper','tool','production','framework','survey','design','system','substantial-bundle','user-probe','interrogation','audit')
      AND NOT (substr(dt.value, 1, 5) = 'iter-'))
ORDER BY d.id;
