-- name: field-references
-- params: name
SELECT f.symbol AS field, f.owner AS owner, f.file AS defined_in, f.line AS defined_line,
       fn.symbol AS function, r.source_file AS file, r.source_line AS line
FROM Field f LEFT JOIN "REFERENCES" r ON r.dst = f.symbol LEFT JOIN Function fn ON fn.symbol = r.src
WHERE instr(f.name, $name) > 0
ORDER BY field, file, line LIMIT 50;
