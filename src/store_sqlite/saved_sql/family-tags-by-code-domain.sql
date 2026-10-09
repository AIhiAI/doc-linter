-- name: family-tags-by-code-domain
-- params: 
WITH rd AS (SELECT d.id FROM Doc d WHERE EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'research') AND NOT EXISTS (SELECT 1 FROM doc_tags WHERE doc_id = d.id AND value = 'index')),
fam AS (SELECT dt.value AS family_tag, count(DISTINCT dt.doc_id) AS member_count
        FROM doc_tags dt JOIN rd ON rd.id = dt.doc_id
        WHERE dt.value NOT IN ('research', 'paper', 'tool', 'production', 'framework', 'survey', 'design', 'system', 'ingest-pass-storyline')
        GROUP BY dt.value HAVING count(DISTINCT dt.doc_id) >= 3),
m AS (
  SELECT f.family_tag, f.member_count, d.id AS did FROM fam f
  JOIN doc_tags t ON t.value = f.family_tag JOIN rd ON rd.id = t.doc_id JOIN Doc d ON d.id = t.doc_id),
agg AS (
  SELECT family_tag, member_count,
    sum(CASE WHEN EXISTS (SELECT 1 FROM doc_tags c WHERE c.doc_id = m.did AND c.value IN ('code-rag', 'code-embedding-lineage', 'code-context-engine', 'code-comprehension', 'static-analysis', 'code-indexer-architecture', 'code-similarity', 'code-embedding', 'ast-paths', 'ast-structural-features')) THEN 1 ELSE 0 END) AS code_count
  FROM m GROUP BY family_tag, member_count)
SELECT family_tag, member_count, code_count,
  CASE WHEN 1.0 * code_count / member_count > 0.5 THEN 'code-domain'
       WHEN 1.0 * code_count / member_count < 0.2 THEN 'generic' ELSE 'mixed' END AS domain_label
FROM agg ORDER BY domain_label, family_tag LIMIT 25;
