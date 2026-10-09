-- name: unified-corpus-diagnostic
-- params: 
WITH c AS (SELECT (SELECT count(*) FROM Doc) AS total_docs, (SELECT count(*) FROM Doc d2 WHERE d2.role IN ('ontology-entity', 'ontology-value', 'ontology-axis', 'ontology-migration', 'index')) AS ontology_meta_docs,
  (SELECT count(*) FROM Entity) AS total_entities, (SELECT count(*) FROM Function) AS total_functions, (SELECT count(*) FROM File) AS total_files,
  (SELECT count(DISTINCT tf.path) FROM File tf WHERE instr(tf.path, 'test_') > 0) AS total_test_files,
  (SELECT count(DISTINCT tfn.symbol) FROM Function tfn WHERE instr(tfn.symbol, 'test_') > 0) AS total_test_functions,
  (SELECT count(DISTINCT fn.symbol) FROM Function fn WHERE instr(fn.symbol, 'test_') > 0 AND instr(fn.symbol, '/_') > 0 AND NOT (instr(fn.symbol, '#__') > 0)) AS module_helper_count,
  (SELECT count(DISTINCT fn.symbol) FROM Function fn WHERE instr(fn.symbol, 'test_') > 0 AND instr(fn.symbol, '#__') > 0 AND NOT (instr(fn.symbol, 'tests.test_') > 0)) AS inner_class_helper_count)
SELECT total_docs, total_entities, total_functions, total_files, total_test_files, total_test_functions,
       module_helper_count, inner_class_helper_count, CASE WHEN total_docs = 0 THEN 'EMPTY' WHEN ontology_meta_docs * 100 / total_docs >= 90 THEN 'ADOPTION-BRANCH' WHEN ontology_meta_docs * 100 / total_docs <= 30 THEN 'CONTENT-RICH' ELSE 'MIXED' END AS corpus_purpose, CASE WHEN module_helper_count = 0 AND inner_class_helper_count = 0 THEN 'EMPTY' WHEN inner_class_helper_count = 0 THEN 'APP-MODULE-ONLY' WHEN module_helper_count = 0 THEN 'INNER-CLASS-ONLY' ELSE 'FRAMEWORK-MIXED' END AS test_fixture_idiom
FROM c;
