-- name: session-context-docs
-- params: 
SELECT d.id AS doc_id, d.kind AS kind, d.title AS title, d.summary AS summary FROM Doc d
WHERE instr(d.id, 'session') > 0 OR instr(d.id, 'handoff') > 0 OR instr(d.id, 'next-') > 0 OR instr(d.id, 'resume') > 0 ORDER BY d.id LIMIT 20;
