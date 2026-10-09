-- name: function-call-count-between-files
-- params: caller_file, callee_file
SELECT count(*) AS call_count FROM "CALLS" r JOIN Function c ON c.symbol = r.src JOIN Function t ON t.symbol = r.dst
WHERE c.file = $caller_file AND t.file = $callee_file AND NOT (instr(c.file, 'tests/') > 0) AND NOT (instr(t.symbol, 'tests/') > 0);
