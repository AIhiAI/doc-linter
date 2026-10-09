-- name: workflow-validations
-- params: 
SELECT d.id AS doc_id, d.title AS title, d.kind AS kind, (SELECT json_group_array(DISTINCT t.value) FROM doc_tags t WHERE t.doc_id = d.id AND t.value IN ('install-setup-workflow', 'churn-analysis-workflow', 'pre-commit-workflow', 'security-review-workflow', 'review-workflow', 'refactor-workflow')) AS workflow_tags
FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags t WHERE t.doc_id = d.id AND t.value IN ('install-setup-workflow', 'churn-analysis-workflow', 'pre-commit-workflow', 'security-review-workflow', 'review-workflow', 'refactor-workflow')) ORDER BY d.id;
