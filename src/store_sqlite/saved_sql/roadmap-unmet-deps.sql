-- name: roadmap-unmet-deps
-- params: 
SELECT a.id AS feature, b.id AS unmet_dep, b.lifecycle AS dep_lifecycle FROM Doc a JOIN "DEPENDS_ON" r ON r.src = a.id JOIN Doc b ON b.id = r.dst
WHERE a.role = 'feature' AND b.role = 'feature' AND b.lifecycle IN ('planning', 'implementing') ORDER BY feature, unmet_dep;
