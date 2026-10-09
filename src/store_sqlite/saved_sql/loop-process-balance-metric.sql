-- name: loop-process-balance-metric
-- params: 
WITH s AS (SELECT coalesce(sum(CASE WHEN substr(d.id, 1, length('user-probe-')) = 'user-probe-' THEN 1 ELSE 0 END), 0) AS user_probe_count,
                  coalesce(sum(CASE WHEN substr(d.id, 1, length('interrogation-')) = 'interrogation-' THEN 1 ELSE 0 END), 0) AS interrogation_count FROM Doc d)
SELECT user_probe_count, interrogation_count,
  CASE WHEN interrogation_count = 0 THEN 0.0 ELSE 1.0 * user_probe_count / interrogation_count END AS user_probe_to_interrogation_ratio,
  CASE WHEN interrogation_count = 0 AND user_probe_count = 0 THEN 'balanced' WHEN interrogation_count = 0 THEN 'V-heavy'
       WHEN user_probe_count = 0 THEN 'I-heavy' WHEN 1.0 * user_probe_count / interrogation_count > 1.2 THEN 'V-heavy'
       WHEN 1.0 * user_probe_count / interrogation_count < 0.83 THEN 'I-heavy' ELSE 'balanced' END AS balance_label FROM s;
