-- name: onboarding-doc-shortlist
-- params: 
SELECT d.id AS doc_id, d.kind AS kind, d.title AS title, d.summary AS summary FROM Doc d
WHERE d.role = 'doc' AND d.kind IN ('explanation','how-to','tutorial') ORDER BY d.kind, d.id LIMIT 20;
