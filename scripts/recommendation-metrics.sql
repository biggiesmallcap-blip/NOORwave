-- Recommendation health metrics. Read-only; run with:
--   sqlite3 -readonly noor.db < scripts/recommendation-metrics.sql
.headers on
.mode list
.separator " | "

SELECT '1. active model' AS section;
SELECT id, created_at, json_extract(config_json, '$.trainer_config_version') AS trainer_version,
       round(json_extract(metrics_json, '$.transition_recall_at_10'), 4) AS recall10,
       round(json_extract(metrics_json, '$.coverage_ratio'), 4) AS coverage
FROM embedding_models WHERE is_active = 1;

SELECT '2. primary reason share, top-10 neighbors' AS section;
SELECT COALESCE(primary_reason, '(none)') AS reason, COUNT(*) AS n,
       round(100.0 * COUNT(*) / SUM(COUNT(*)) OVER (), 2) AS pct
FROM track_neighbors
WHERE model_id = (SELECT id FROM embedding_models WHERE is_active = 1) AND rank <= 10
GROUP BY 1 ORDER BY n DESC;

SELECT '3. behavioral score vs co-listen support' AS section;
SELECT support_colisten > 0 AS has_colisten, COUNT(*) AS n, round(AVG(behavioral_score), 4) AS avg_behavioral
FROM track_neighbors
WHERE model_id = (SELECT id FROM embedding_models WHERE is_active = 1)
GROUP BY 1;

SELECT '4. top-10 neighbors outside the library' AS section;
SELECT round(100.0 * SUM(COALESCE(t.is_library, 0) = 0) / COUNT(*), 2) AS pct_non_library
FROM track_neighbors n JOIN tracks t ON t.id = n.neighbor_track_id
WHERE n.model_id = (SELECT id FROM embedding_models WHERE is_active = 1) AND n.rank <= 10;

SELECT '5. track_similarity coverage' AS section;
SELECT COUNT(*) AS pairs,
       SUM(co_listen_score > 0) AS colisten_pairs,
       SUM(genre_proximity > 0) AS genre_pairs,
       SUM(co_album_score = 0 AND co_artist_score = 0 AND co_listen_score = 0) AS genre_only_pairs,
       COUNT(DISTINCT CASE WHEN co_album_score = 0 AND co_artist_score = 0 AND co_listen_score = 0 THEN track_a END) AS genre_only_seed_tracks,
       round(AVG(co_artist_score), 3) AS avg_co_artist,
       round(AVG(co_album_score), 3) AS avg_co_album,
       round(AVG(genre_proximity), 3) AS avg_genre
FROM track_similarity;

SELECT '6. sample seeds, learned top-5' AS section;
SELECT s.title AS seed, n.rank, COALESCE(ar.name, '?') || ' - ' || c.title AS neighbor, n.primary_reason
FROM tracks s
JOIN track_neighbors n ON n.track_id = s.id
     AND n.model_id = (SELECT id FROM embedding_models WHERE is_active = 1) AND n.rank <= 5
JOIN tracks c ON c.id = n.neighbor_track_id
LEFT JOIN artists ar ON ar.id = c.artist_id
WHERE s.id IN (
    SELECT (SELECT MIN(t.id) FROM tracks t
            WHERE t.is_library = 1 AND t.play_count > 0 AND t.title LIKE p.pattern)
    FROM (SELECT '%Clair de lune%' AS pattern UNION ALL SELECT '%r Elise%'
          UNION ALL SELECT 'Kathy''s Song%' UNION ALL SELECT 'Anniversary'
          UNION ALL SELECT 'La Carretera%') p
)
ORDER BY s.title, n.rank;

SELECT '7. listen outcome by source, last 90 days' AS section;
SELECT CASE
         WHEN lh.source LIKE 'automix%' THEN 'automix'
         WHEN lh.source LIKE 'radio%' THEN 'radio'
         ELSE COALESCE(lh.source, 'manual')
       END AS src,
       COUNT(*) AS listens,
       round(100.0 * SUM(lh.completed = 1) / COUNT(*), 1) AS completed_pct,
       round(100.0 * SUM(COALESCE(lh.duration_listened_ms, 0) < 30000 AND lh.completed = 0) / COUNT(*), 1) AS early_skip_pct
FROM listen_history lh
WHERE julianday(lh.started_at) >= julianday('now', '-90 days')
GROUP BY 1 ORDER BY listens DESC;
