-- Analytics signals spot-check — run against a representative DB to validate
-- the claims the new /api/analytics/signals endpoint will make.
--
-- Usage:
--   sqlite3 path/to/noor.sqlite < noor-server/tests/manual/analytics-spot-check.sql
--
-- Each section produces a small result set you can hand-compare against the
-- corresponding section of the JSON response from
--   curl http://localhost:17600/api/analytics/signals?days=30 | jq
--
-- The window throughout this file is 30 days. Edit the `30` literals in place
-- to spot-check other windows. The branch decisions for the Phase 1 gate are
-- pinned by section 1 (session_id NULL fraction).

.mode column
.headers on
.width 28 12

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. session_id NULL fraction — drives the Phase 1 sessions decision.
--    < 1%   → footnote-tier caption, no further action.
--    1–10%  → "Excludes {N} listens from before sessions were tracked." caption.
--    > 10%  → pause Phase 1; revisit backfill or 30-min gap heuristic.
-- ─────────────────────────────────────────────────────────────────────────────

SELECT 'session_id NULL fraction' AS metric;
SELECT
  COUNT(*)                                            AS total_listens,
  COUNT(*) FILTER (WHERE session_id IS NULL)          AS untracked,
  ROUND(
    100.0 * COUNT(*) FILTER (WHERE session_id IS NULL) / NULLIF(COUNT(*), 0),
    2
  )                                                   AS pct_untracked
FROM listen_history;

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. KPI strip reconciliation (current + previous 30d windows).
--    These four numbers must equal kpis.{listened_ms,sessions,completion,
--    skip_rate}.{current,previous} byte-for-byte (after the i64 → f64 % math).
-- ─────────────────────────────────────────────────────────────────────────────

SELECT 'KPI: listened_ms' AS metric;
SELECT
  COALESCE(SUM(duration_listened_ms) FILTER (
    WHERE started_at >= datetime('now', '-30 days')
  ), 0)                                                                AS cur_ms,
  COALESCE(SUM(duration_listened_ms) FILTER (
    WHERE started_at >= datetime('now', '-60 days')
      AND started_at <  datetime('now', '-30 days')
  ), 0)                                                                AS prev_ms
FROM listen_history;

SELECT 'KPI: sessions (DISTINCT session_id, NOT NULL)' AS metric;
SELECT
  COUNT(DISTINCT CASE
    WHEN started_at >= datetime('now', '-30 days')
      AND session_id IS NOT NULL
    THEN session_id END)                                               AS cur_sessions,
  COUNT(DISTINCT CASE
    WHEN started_at >= datetime('now', '-60 days')
      AND started_at <  datetime('now', '-30 days')
      AND session_id IS NOT NULL
    THEN session_id END)                                               AS prev_sessions
FROM listen_history;

SELECT 'KPI: completion (cur_completed / cur_total)' AS metric;
SELECT
  COUNT(*) FILTER (WHERE started_at >= datetime('now', '-30 days'))    AS cur_listens,
  COUNT(*) FILTER (WHERE started_at >= datetime('now', '-30 days')
                     AND completed = 1)                                AS cur_completed,
  ROUND(
    1.0 * COUNT(*) FILTER (WHERE started_at >= datetime('now', '-30 days')
                             AND completed = 1)
        / NULLIF(COUNT(*) FILTER (WHERE started_at >= datetime('now', '-30 days')), 0),
    4
  )                                                                    AS cur_completion_ratio
FROM listen_history;

-- skip_rate = 1 - completion (computed in Rust); just confirm the cur ratio above.

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Hero stats (peak_hour, night_share, morning_share, distinct_tracks).
--    Rhythm is a CV calculation across per-day stddev — easier to spot-check
--    by inspecting the per-day per-hour shape directly than to re-derive the
--    formula in SQL.
-- ─────────────────────────────────────────────────────────────────────────────

SELECT 'Hero: peak_hour (max listens, tie earliest)' AS metric;
SELECT
  CAST(strftime('%H', started_at) AS INTEGER) AS hour,
  COUNT(*)                                    AS listens
FROM listen_history
WHERE started_at >= datetime('now', '-30 days')
GROUP BY hour
ORDER BY listens DESC, hour ASC
LIMIT 5;

SELECT 'Hero: night/morning share (Night 22-04, Morning 05-09)' AS metric;
SELECT
  COUNT(*)                                                            AS total,
  COUNT(*) FILTER (WHERE CAST(strftime('%H', started_at) AS INTEGER)
                          IN (22, 23, 0, 1, 2, 3, 4))                AS night_listens,
  COUNT(*) FILTER (WHERE CAST(strftime('%H', started_at) AS INTEGER)
                          IN (5, 6, 7, 8, 9))                         AS morning_listens,
  ROUND(1.0 * COUNT(*) FILTER (WHERE CAST(strftime('%H', started_at) AS INTEGER)
                                       IN (22, 23, 0, 1, 2, 3, 4))
            / NULLIF(COUNT(*), 0), 4)                                 AS night_share,
  ROUND(1.0 * COUNT(*) FILTER (WHERE CAST(strftime('%H', started_at) AS INTEGER)
                                       IN (5, 6, 7, 8, 9))
            / NULLIF(COUNT(*), 0), 4)                                 AS morning_share
FROM listen_history
WHERE started_at >= datetime('now', '-30 days');

SELECT 'Hero: days-with-listens count (Rhythm renders -- when < 5)' AS metric;
SELECT COUNT(DISTINCT DATE(started_at)) AS active_days
FROM listen_history
WHERE started_at >= datetime('now', '-30 days');

-- ─────────────────────────────────────────────────────────────────────────────
-- 4. Tempo: per-listen BPM vector reconciliation.
--    SUM over dense-filled buckets must equal the count of listens with
--    bpm IN [60, 200) joined to audio_dsp_features.
-- ─────────────────────────────────────────────────────────────────────────────

SELECT 'Tempo: total listens with BPM in [60, 200)' AS metric;
SELECT COUNT(*) AS analysed_listens
FROM listen_history lh
JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
WHERE lh.started_at >= datetime('now', '-30 days')
  AND adf.bpm >= 60 AND adf.bpm < 200;

SELECT 'Tempo: per-listen BPM stats (median, mode bucket, sigma proxy)' AS metric;
WITH per_listen AS (
  SELECT adf.bpm
  FROM listen_history lh
  JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
  WHERE lh.started_at >= datetime('now', '-30 days')
    AND adf.bpm >= 60 AND adf.bpm < 200
)
SELECT
  ROUND(AVG(bpm), 1)                                       AS mean_bpm,
  ROUND(
    SQRT(AVG((bpm - (SELECT AVG(bpm) FROM per_listen)) *
             (bpm - (SELECT AVG(bpm) FROM per_listen)))),
    2
  )                                                        AS sigma_bpm
FROM per_listen;
-- Note: SQLite has no stddev or percentile_cont, so median is omitted here.
-- Compare the Rust-side median to the response value separately.

SELECT 'Tempo: bucket distribution (sanity check shape vs response)' AS metric;
SELECT
  (CAST(adf.bpm AS INTEGER) / 4) * 4 AS bucket,
  COUNT(*)                            AS listens
FROM listen_history lh
JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
WHERE lh.started_at >= datetime('now', '-30 days')
  AND adf.bpm >= 60 AND adf.bpm < 200
GROUP BY bucket
ORDER BY listens DESC
LIMIT 10;

-- ─────────────────────────────────────────────────────────────────────────────
-- 5. Coverage: how many listened tracks have DSP analysis?
--    Surfaces the {analyzed} / {total_listened} captions on Tempo, Sonic Field,
--    Audio Profile.
-- ─────────────────────────────────────────────────────────────────────────────

SELECT 'Coverage: analysed / total listened tracks (window)' AS metric;
WITH listened AS (
  SELECT DISTINCT track_id
  FROM listen_history
  WHERE started_at >= datetime('now', '-30 days')
)
SELECT
  COUNT(*)                                                              AS total_listened,
  COUNT(*) FILTER (WHERE EXISTS (
    SELECT 1 FROM audio_dsp_features adf WHERE adf.track_id = listened.track_id
  ))                                                                    AS analysed,
  ROUND(
    100.0 * COUNT(*) FILTER (WHERE EXISTS (
      SELECT 1 FROM audio_dsp_features adf WHERE adf.track_id = listened.track_id
    )) / NULLIF(COUNT(*), 0),
    1
  )                                                                     AS pct_analysed
FROM listened;

-- ─────────────────────────────────────────────────────────────────────────────
-- 6. Cohorts sanity check (disjoint definitions partition the window).
--    The three counts MUST sum to the total of listened-in-window distinct
--    tracks. If they don't, the cohort SQL has a bug.
-- ─────────────────────────────────────────────────────────────────────────────

SELECT 'Cohorts: disjoint partition check' AS metric;
WITH first_listens AS (
  SELECT track_id, MIN(started_at) AS first_at, COUNT(*) AS lifetime_listens
  FROM listen_history
  GROUP BY track_id
),
windowed_tracks AS (
  SELECT DISTINCT lh.track_id
  FROM listen_history lh
  WHERE lh.started_at >= datetime('now', '-30 days')
)
SELECT
  COUNT(*) FILTER (WHERE fl.first_at >= datetime('now', '-30 days'))                                        AS new_this_month,
  COUNT(*) FILTER (WHERE fl.first_at <  datetime('now', '-30 days')
                     AND NOT (fl.first_at < datetime('now', '-180 days') AND fl.lifetime_listens >= 5))     AS established,
  COUNT(*) FILTER (WHERE fl.first_at <  datetime('now', '-180 days')
                     AND fl.lifetime_listens >= 5)                                                          AS deep_cuts,
  COUNT(*)                                                                                                  AS total_windowed
FROM windowed_tracks wt
JOIN first_listens fl USING (track_id);
-- Expectation: new_this_month + established + deep_cuts == total_windowed.

-- ─────────────────────────────────────────────────────────────────────────────
-- 7. Audio profile: loudness mean / dynamic range proxy / spectral-centroid mean.
-- ─────────────────────────────────────────────────────────────────────────────

SELECT 'Audio profile: weighted means over listened analysed tracks' AS metric;
SELECT
  ROUND(SUM(adf.loudness_lufs)     / NULLIF(COUNT(*), 0), 2) AS mean_loudness_lufs,
  ROUND(MIN(adf.loudness_lufs), 2)                            AS min_loudness,
  ROUND(MAX(adf.loudness_lufs), 2)                            AS max_loudness,
  ROUND(SUM(adf.spectral_centroid) / NULLIF(COUNT(*), 0), 0) AS mean_spectral_centroid_hz,
  COUNT(*)                                                    AS analysed_listens
FROM listen_history lh
JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
WHERE lh.started_at >= datetime('now', '-30 days');
-- Note: loudness here is per-listen weighted (every play counts) — same vector the
-- backend uses. Dynamic range is computed in Rust as P95(loudness) - P5(loudness).
