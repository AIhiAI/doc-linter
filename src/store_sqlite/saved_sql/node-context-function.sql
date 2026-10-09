-- name: node-context-function
-- params: symbol
SELECT centre.symbol AS symbol, centre.file AS file, centre.line AS line, centre.doc_comment AS doc_comment, centre.signature AS signature,
  (SELECT json_group_array(json_object('kind','doc','id',x.id,'title',x.title,'summary',x.summary,'path',x.path)) FROM (SELECT DISTINCT d.id, d.title, d.summary, d.path FROM "FUNCTION_DEFINED_IN" r JOIN Doc d ON d.id = r.dst WHERE r.src = centre.symbol) x) AS docs,
  (SELECT json_group_array(json_object('kind','entity','id',x.id,'display',x.display,'description',x.description)) FROM (SELECT DISTINCT e.id, e.display, e.description FROM "FUNCTION_BELONGS_TO" r JOIN Entity e ON e.id = r.dst WHERE r.src = centre.symbol) x) AS entities_belongs,
  (SELECT json_group_array(json_object('kind','entity','id',x.id,'display',x.display,'description',x.description,'confidence',x.confidence)) FROM (SELECT DISTINCT e.id, e.display, e.description, r.confidence FROM "FUNCTION_MENTIONS" r JOIN Entity e ON e.id = r.dst WHERE r.src = centre.symbol) x) AS entities_mentions,
  (SELECT json_group_array(json_object('kind','type','symbol',x.symbol,'doc_comment',x.doc_comment)) FROM (SELECT DISTINCT t.symbol, t.doc_comment FROM "METHOD_OF" r JOIN Type t ON t.symbol = r.dst WHERE r.src = centre.symbol) x) AS method_of,
  (SELECT json_group_array(json_object('symbol',x.symbol,'file',x.file,'line',x.line,'doc_comment',x.doc_comment)) FROM (SELECT DISTINCT c.symbol, c.file, c.line, c.doc_comment FROM "CALLS" r JOIN Function c ON c.symbol = r.dst WHERE r.src = centre.symbol) x) AS calls_out,
  (SELECT json_group_array(json_object('symbol',x.symbol,'file',x.file,'line',x.line)) FROM (SELECT DISTINCT c.symbol, c.file, c.line FROM "CALLS" r JOIN Function c ON c.symbol = r.src WHERE r.dst = centre.symbol) x) AS calls_in
FROM Function centre WHERE centre.symbol = $symbol;
