-- name: mcp-handlers
-- params: 
SELECT e.path AS tool_name, e.handler_symbol AS handler_symbol, f.symbol AS function_symbol, f.file AS file, f.line AS line
FROM Endpoint e JOIN "ENDPOINT_HANDLED_BY" h ON h.src = e.id JOIN Function f ON f.symbol = h.dst
WHERE e.kind = 'mcp' ORDER BY e.path;
