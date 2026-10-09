-- name: function-shape-by-file
-- params: 
WITH base AS (
  SELECT target.symbol AS symbol, target.file AS file FROM Function target
  WHERE NOT (instr(target.file, 'tests/') > 0) AND NOT (instr(target.symbol, 'tests/') > 0)),
inb AS (
  SELECT b.symbol, b.file, count(DISTINCT c.symbol) AS cc, count(DISTINCT c.file) AS cfc
  FROM base b LEFT JOIN "CALLS" r ON r.dst = b.symbol
  LEFT JOIN Function c ON c.symbol = r.src AND c.symbol <> b.symbol
  GROUP BY b.symbol),
outb AS (
  SELECT i.symbol, i.file, i.cc, i.cfc, count(DISTINCT e.symbol) AS ec, count(DISTINCT e.file) AS efc
  FROM inb i LEFT JOIN "CALLS" r ON r.src = i.symbol
  LEFT JOIN Function e ON e.symbol = r.dst AND e.symbol <> i.symbol
  WHERE i.cc >= 5 GROUP BY i.symbol),
sh AS (SELECT symbol, file, cc, cfc, ec, efc, CASE WHEN cc >= 5 AND cfc = 1 AND ec = 0 THEN 'pure-leaf-factory' WHEN cc >= 5 AND cfc >= 2 AND ec = 0 THEN 'wide-leaf-utility' WHEN cc >= 5 AND cfc >= 3 AND ec >= 1 AND ec <= 2 THEN 'thin-entry-point' WHEN cc >= 5 AND cfc = 1 AND ec >= 1 THEN 'subsystem-hub' WHEN cc >= 5 AND cfc >= 2 AND ec >= 5 THEN 'extension-point' ELSE 'peripheral' END AS shape FROM outb)
SELECT file, shape, count(symbol) AS function_count FROM sh
GROUP BY file, shape ORDER BY file, function_count DESC, shape;
