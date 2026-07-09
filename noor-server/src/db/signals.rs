use super::models::*;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

// --- Analytics Signals ------------------------------------------------------
// Backend for GET /api/analytics/signals - the visual-overhaul analytics page.
// Spec: noor-server/tests/fixtures/signals-schema.json
// Spot-check: noor-server/tests/manual/analytics-spot-check.sql

const BPM_MIN: i32 = 60;
const BPM_MAX: i32 = 200;
const BPM_STEP: i32 = 4;
/// Dense bucket count = (max - min) / step. Buckets cover [60, 64, ..., 196] - 35 entries.
/// The TempoRow.buckets array always has exactly this length.
const BPM_BUCKET_COUNT: usize = ((BPM_MAX - BPM_MIN) / BPM_STEP) as usize;

// Night = 22:00-04:00 inclusive (7 hours). Morning = 05:00-09:00 inclusive (5 hours).
// The hour lists are inlined into the SQL where they're consumed (see get_signals_hero_stats);
// the labels live there as the canonical source of truth.

const SONIC_FIELD_LIMIT: i64 = 1500;
const COHORT_NEW_DAYS: i64 = 30;
const COHORT_DEEP_DAYS: i64 = 180;
const COHORT_DEEP_LIFETIME_LISTENS: i64 = 5;
const MONTH_ROW_CAP: usize = 24;
const RIDGELINE_DAY_CAP: i64 = 365;

/// SQL fragment: a listen row's duration, capped at the track's length when
/// the length is known (NULL or 0 duration_ms falls back to the raw value).
///
/// The listen-session timer accrues wall-clock time while the player is
/// nominally playing, so stalled streams recorded runaway rows (observed:
/// 2795 s "listened" on a 334 s track). The writer now clamps new rows
/// (player::clamp_listened_ms); this expression self-heals the historical
/// rows already in users' DBs everywhere this page sums listened time.
fn capped_listened_ms(listen_alias: &str, track_alias: &str) -> String {
    format!(
        "MIN({l}.duration_listened_ms, COALESCE(NULLIF({t}.duration_ms, 0), {l}.duration_listened_ms))",
        l = listen_alias,
        t = track_alias
    )
}

/// SQL fragment: keep only listens the user chose to play. Radio and automix
/// pick tracks by themselves, so counting them in the taste-ranked cards made
/// "Top Artists" reflect the radio's choices (12 consecutive radio plays of
/// one artist outranked everything the user actually picked). NULL / legacy /
/// unknown sources stay included - their provenance is unknowable.
const CHOSEN_LISTENS_ONLY: &str = "COALESCE(lh.source, '') NOT IN ('radio', 'automix')";

/// Granularity selection - locked fallback rule.
///
///   1..=7   -> Day (always)
///   8..=30  -> Day by default; fall back to Week when ridges would be mostly empty:
///             distinct_days < 15 OR median listens-per-day < 5
///   31..=90 -> Week
///   _       -> Month (capped at 24 rows downstream)
fn select_granularity(conn: &Connection, days: i64) -> Result<Granularity> {
    let base = match days {
        1..=7 => Granularity::Day,
        8..=30 => Granularity::Day,
        31..=90 => Granularity::Week,
        _ => Granularity::Month,
    };
    if !(8..=30).contains(&days) {
        return Ok(base);
    }
    let (distinct_days, median_per_day) = compute_30d_density(conn, days)?;
    if distinct_days < 15 || median_per_day < 5.0 {
        Ok(Granularity::Week)
    } else {
        Ok(base)
    }
}

fn compute_30d_density(conn: &Connection, days: i64) -> Result<(i64, f64)> {
    let distinct_days: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT DATE(started_at))
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))",
        params![days],
        |row| row.get(0),
    )?;
    if distinct_days == 0 {
        return Ok((0, 0.0));
    }
    let mut stmt = conn.prepare(
        "SELECT COUNT(*) FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY DATE(started_at)
         ORDER BY COUNT(*) ASC",
    )?;
    let counts: Vec<i64> = stmt
        .query_map(params![days], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<_>>()?;
    let median = median_of_sorted(&counts).unwrap_or(0.0);
    Ok((distinct_days, median))
}

fn median_of_sorted(sorted: &[i64]) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let n = sorted.len();
    if n % 2 == 1 {
        Some(sorted[n / 2] as f64)
    } else {
        let mid = n / 2;
        Some((sorted[mid - 1] + sorted[mid]) as f64 / 2.0)
    }
}

/// Rhythm CV formula. Returns None if fewer than 5 days have any listens.
///
/// For each day d in window:
///   sigma_d = stddev(listens per hour across 24 hours of day d)
/// mean_sigma = average of sigma_d over days with listens
/// mean_listens = mean hourly listens across window (per hour-slot, total/days/24)
/// cv = mean_sigma / mean_listens
/// rhythm = round(100 * clamp(1 - cv, 0, 1))
///
/// CV form so a quiet week and a busy week with the same routine score identically.
fn compute_rhythm(per_day_per_hour: &[[i64; 24]]) -> Option<i32> {
    let active: Vec<&[i64; 24]> = per_day_per_hour
        .iter()
        .filter(|hours| hours.iter().any(|&h| h > 0))
        .collect();
    if active.len() < 5 {
        return None;
    }
    let mut total_listens: f64 = 0.0;
    let mut sigma_sum: f64 = 0.0;
    for hours in &active {
        let mean = hours.iter().map(|&h| h as f64).sum::<f64>() / 24.0;
        let var = hours
            .iter()
            .map(|&h| {
                let diff = h as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / 24.0;
        sigma_sum += var.sqrt();
        total_listens += mean * 24.0;
    }
    let mean_sigma = sigma_sum / active.len() as f64;
    let mean_listens_per_hour = total_listens / (active.len() as f64 * 24.0);
    if mean_listens_per_hour == 0.0 {
        return Some(0);
    }
    let cv = mean_sigma / mean_listens_per_hour;
    let rhythm = (100.0 * (1.0 - cv).clamp(0.0, 1.0)).round() as i32;
    Some(rhythm)
}

/// Listen-weighted median over (bpm, listens) pairs. Mathematically identical to
/// expanding to a per-listen vector and taking its median, without the memory cost.
fn weighted_median_bpm(weighted: &[(f64, i64)]) -> Option<f64> {
    if weighted.is_empty() {
        return None;
    }
    let mut pairs: Vec<(f64, i64)> = weighted.to_vec();
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let total: i64 = pairs.iter().map(|(_, w)| *w).sum();
    if total == 0 {
        return None;
    }
    let half = total as f64 / 2.0;
    let mut cum: f64 = 0.0;
    for (i, (bpm, w)) in pairs.iter().enumerate() {
        let next = cum + *w as f64;
        if next >= half {
            // For even totals where we land exactly on the boundary, average with the next.
            if (next - half).abs() < f64::EPSILON && i + 1 < pairs.len() {
                return Some((bpm + pairs[i + 1].0) / 2.0);
            }
            return Some(*bpm);
        }
        cum = next;
    }
    pairs.last().map(|(b, _)| *b)
}

/// Listen-weighted stddev over (bpm, listens) pairs.
fn weighted_stddev_bpm(weighted: &[(f64, i64)]) -> Option<f64> {
    let total: i64 = weighted.iter().map(|(_, w)| *w).sum();
    if total < 2 {
        return None;
    }
    let total_f = total as f64;
    let mean: f64 = weighted.iter().map(|(b, w)| b * *w as f64).sum::<f64>() / total_f;
    let variance: f64 = weighted
        .iter()
        .map(|(b, w)| {
            let diff = b - mean;
            diff * diff * (*w as f64)
        })
        .sum::<f64>()
        / total_f;
    Some(variance.sqrt())
}

/// Window bookkeeping for the analytics response.
fn build_signals_window(
    conn: &Connection,
    days: i64,
    granularity: Granularity,
) -> Result<SignalsWindow> {
    let (started_at, previous_started_at, generated_at): (String, String, String) = conn
        .query_row(
            "SELECT
                strftime('%Y-%m-%dT%H:%M:%SZ', datetime('now', printf('-%d days', ?1))),
                strftime('%Y-%m-%dT%H:%M:%SZ', datetime('now', printf('-%d days', ?2))),
                strftime('%Y-%m-%dT%H:%M:%SZ', datetime('now'))",
            params![days, days * 2],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

    Ok(SignalsWindow {
        days,
        started_at,
        previous_started_at,
        generated_at,
        granularity,
        display_caps: DisplayCaps {
            ridgeline_days: (days > RIDGELINE_DAY_CAP).then_some(RIDGELINE_DAY_CAP),
            tempo_rows: match granularity {
                Granularity::Day if days > RIDGELINE_DAY_CAP => Some(RIDGELINE_DAY_CAP),
                Granularity::Month if days > (MONTH_ROW_CAP as i64 * 31) => {
                    Some(MONTH_ROW_CAP as i64)
                }
                _ => None,
            },
        },
    })
}

fn get_analytics_totals(conn: &Connection, days: i64) -> Result<AnalyticsTotals> {
    let sql = format!(
        "SELECT
            COUNT(lh.id),
            COALESCE(SUM({capped}), 0),
            COUNT(DISTINCT lh.track_id),
            COUNT(lh.id) FILTER (
                WHERE EXISTS (
                    SELECT 1 FROM track_genres tg WHERE tg.track_id = lh.track_id
                )
            )
         FROM listen_history lh
         LEFT JOIN tracks t ON t.id = lh.track_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))",
        capped = capped_listened_ms("lh", "t")
    );
    conn.query_row(&sql, params![days], |row| {
        Ok(AnalyticsTotals {
            listens: row.get(0)?,
            listened_ms: row.get(1)?,
            distinct_tracks: row.get(2)?,
            tagged_listens: row.get(3)?,
        })
    })
    .map_err(Into::into)
}

// --- KPI window: listened_ms / sessions / completion / skip_rate (cur+prev) --

fn get_signals_kpis(conn: &Connection, days: i64) -> Result<SignalsKpis> {
    let cur_offset = days;
    let prev_offset = days * 2;

    let window_sql = format!(
        "SELECT
            COUNT(*) FILTER (WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))),
            COUNT(*) FILTER (WHERE lh.started_at >= datetime('now', printf('-%d days', ?1)) AND lh.completed = 1),
            COALESCE(SUM({capped}) FILTER (WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))), 0),
            COUNT(*) FILTER (WHERE lh.started_at >= datetime('now', printf('-%d days', ?2)) AND lh.started_at < datetime('now', printf('-%d days', ?1))),
            COUNT(*) FILTER (WHERE lh.started_at >= datetime('now', printf('-%d days', ?2)) AND lh.started_at < datetime('now', printf('-%d days', ?1)) AND lh.completed = 1),
            COALESCE(SUM({capped}) FILTER (WHERE lh.started_at >= datetime('now', printf('-%d days', ?2)) AND lh.started_at < datetime('now', printf('-%d days', ?1))), 0)
         FROM listen_history lh
         LEFT JOIN tracks t ON t.id = lh.track_id",
        capped = capped_listened_ms("lh", "t")
    );
    let (cur_listens, cur_completed, cur_ms, prev_listens, prev_completed, prev_ms): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = conn.query_row(&window_sql, params![cur_offset, prev_offset], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        ))
    })?;

    // Sessions (post-MIGRATION_023 only - session_id IS NULL on older history).
    let (cur_sessions, prev_sessions): (i64, i64) = conn.query_row(
        "SELECT
            COUNT(DISTINCT CASE WHEN started_at >= datetime('now', printf('-%d days', ?1)) AND session_id IS NOT NULL THEN session_id END),
            COUNT(DISTINCT CASE WHEN started_at >= datetime('now', printf('-%d days', ?2)) AND started_at < datetime('now', printf('-%d days', ?1)) AND session_id IS NOT NULL THEN session_id END)
         FROM listen_history",
        params![cur_offset, prev_offset],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let (sessions_tracked, sessions_untracked): (i64, i64) = conn.query_row(
        "SELECT
            COUNT(*) FILTER (WHERE session_id IS NOT NULL),
            COUNT(*) FILTER (WHERE session_id IS NULL)
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))",
        params![cur_offset],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    // Daily series for the MiniSilhouette curves.
    let daily_sql = format!(
        "WITH RECURSIVE axis(d) AS (
            SELECT DATE(datetime('now', 'localtime', printf('-%d days', ?1 - 1)))
            UNION ALL
            SELECT DATE(d, '+1 day') FROM axis WHERE d < DATE('now', 'localtime')
         ),
         agg AS (
            SELECT
                DATE(lh.started_at, 'localtime') AS day,
                COUNT(*) AS listens,
                COALESCE(SUM({capped}), 0) AS listened_ms,
                COALESCE(SUM(CASE WHEN lh.completed = 1 THEN 1 ELSE 0 END), 0) AS completed,
                COUNT(DISTINCT CASE WHEN lh.session_id IS NOT NULL THEN lh.session_id END) AS sessions
            FROM listen_history lh
            LEFT JOIN tracks t ON t.id = lh.track_id
            WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
            GROUP BY DATE(lh.started_at, 'localtime')
         )
         SELECT
            axis.d,
            COALESCE(agg.listens, 0),
            COALESCE(agg.listened_ms, 0),
            COALESCE(agg.completed, 0),
            COALESCE(agg.sessions, 0)
         FROM axis
         LEFT JOIN agg ON agg.day = axis.d
         ORDER BY axis.d ASC",
        capped = capped_listened_ms("lh", "t")
    );
    let mut daily_stmt = conn.prepare(&daily_sql)?;
    let daily: Vec<DailyKpi> = daily_stmt
        .query_map(params![cur_offset], |row| {
            Ok(DailyKpi {
                day: row.get(0)?,
                listens: row.get(1)?,
                listened_ms: row.get(2)?,
                completed: row.get(3)?,
                sessions: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    let completion = ratio_or_none(cur_completed, cur_listens);
    let prev_completion = ratio_or_none(prev_completed, prev_listens);

    let kpis = SignalsKpis {
        listened_ms: KpiPairInt {
            current: cur_ms,
            previous: prev_ms,
        },
        sessions: KpiPairInt {
            current: cur_sessions,
            previous: prev_sessions,
        },
        completion: KpiPairFloat {
            current: completion,
            previous: prev_completion,
        },
        skip_rate: KpiPairFloat {
            current: completion.map(|c| 1.0 - c),
            previous: prev_completion.map(|c| 1.0 - c),
        },
        daily,
        hero_stats: get_signals_hero_stats(conn, days)?,
        sessions_coverage: SessionsCoverage {
            tracked: sessions_tracked,
            untracked: sessions_untracked,
        },
    };

    Ok(kpis)
}

fn ratio_or_none(num: i64, denom: i64) -> Option<f64> {
    if denom == 0 {
        None
    } else {
        Some(num as f64 / denom as f64)
    }
}

// --- Hero stats -------------------------------------------------------------

fn get_signals_hero_stats(conn: &Connection, days: i64) -> Result<HeroStats> {
    // Peak hour: hour-of-day with max total listens, tie-break earliest.
    let peak_hour: Option<i32> = conn
        .query_row(
            "SELECT CAST(strftime('%H', started_at, 'localtime') AS INTEGER) AS h
             FROM listen_history
             WHERE started_at >= datetime('now', printf('-%d days', ?1))
             GROUP BY h
             ORDER BY COUNT(*) DESC, h ASC
             LIMIT 1",
            params![days],
            |row| row.get::<_, i32>(0),
        )
        .optional()?;

    // Per-day per-hour matrix (zero-filled) for Rhythm.
    let mut hour_stmt = conn.prepare(
        "SELECT DATE(started_at, 'localtime') AS day, CAST(strftime('%H', started_at, 'localtime') AS INTEGER) AS h, COUNT(*) AS c
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY day, h
         ORDER BY day, h",
    )?;
    let mut day_map: HashMap<String, [i64; 24]> = HashMap::new();
    let rows = hour_stmt.query_map(params![days], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i32>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for r in rows {
        let (day, h, c) = r?;
        let entry = day_map.entry(day).or_insert([0i64; 24]);
        if (0..24).contains(&h) {
            entry[h as usize] = c;
        }
    }
    let per_day: Vec<[i64; 24]> = day_map.values().copied().collect();
    let rhythm = compute_rhythm(&per_day);

    // Night / Morning shares - None when there are no listens in window.
    let (total, night, morning): (i64, i64, i64) = conn.query_row(
        "SELECT
            COUNT(*),
            COUNT(*) FILTER (WHERE CAST(strftime('%H', started_at, 'localtime') AS INTEGER) IN (22, 23, 0, 1, 2, 3, 4)),
            COUNT(*) FILTER (WHERE CAST(strftime('%H', started_at, 'localtime') AS INTEGER) IN (5, 6, 7, 8, 9))
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))",
        params![days],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let (night_share, morning_share) = if total == 0 {
        (None, None)
    } else {
        (
            Some(night as f64 / total as f64),
            Some(morning as f64 / total as f64),
        )
    };

    // Single-day mode (days <= 1) populates the two extra spine stats.
    let (longest_session_ms, distinct_tracks) = if days <= 1 {
        let longest_sql = format!(
            "SELECT MAX(session_total) FROM (
                 SELECT lh.session_id, SUM({capped}) AS session_total
                 FROM listen_history lh
                 LEFT JOIN tracks t ON t.id = lh.track_id
                 WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
                   AND lh.session_id IS NOT NULL
                 GROUP BY lh.session_id
             )",
            capped = capped_listened_ms("lh", "t")
        );
        let longest: Option<i64> = conn
            .query_row(&longest_sql, params![days], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .optional()?
            .flatten();
        let distinct: Option<i64> = conn
            .query_row(
                "SELECT COUNT(DISTINCT track_id)
                 FROM listen_history
                 WHERE started_at >= datetime('now', printf('-%d days', ?1))",
                params![days],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        (longest, distinct)
    } else {
        (None, None)
    };

    Ok(HeroStats {
        peak_hour: if total == 0 { None } else { peak_hour },
        rhythm,
        night_share,
        morning_share,
        longest_session_ms,
        distinct_tracks,
    })
}

// --- Tempo ------------------------------------------------------------------

fn get_signals_tempo(conn: &Connection, days: i64, granularity: Granularity) -> Result<TempoView> {
    // Per-row x per-bucket aggregation (label, bucket, listens) over the window.
    let label_expr = match granularity {
        Granularity::Day => "DATE(lh.started_at, 'localtime')",
        Granularity::Week => "strftime('%Y-%U', lh.started_at, 'localtime')", // %U = Sunday-start (NOT %W)
        Granularity::Month => "strftime('%Y-%m', lh.started_at, 'localtime')",
    };
    let sql = format!(
        "SELECT
            {label_expr} AS label,
            (CAST(adf.bpm AS INTEGER) / {step}) * {step} AS bucket,
            COUNT(*) AS listens
         FROM listen_history lh
         JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
           AND adf.bpm >= {min} AND adf.bpm < {max}
         GROUP BY label, bucket
         ORDER BY label, bucket",
        label_expr = label_expr,
        step = BPM_STEP,
        min = BPM_MIN,
        max = BPM_MAX,
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(String, i32, i64)> = stmt
        .query_map(params![days], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    // Group by label, dense-fill buckets.
    let mut per_label: Vec<(String, Vec<BpmBucket>)> = Vec::new();
    let mut current: Option<(String, Vec<BpmBucket>)> = None;
    for (label, bucket, listens) in &rows {
        if current.as_ref().map(|(l, _)| l) != Some(label) {
            if let Some(prev) = current.take() {
                per_label.push(prev);
            }
            current = Some((label.clone(), dense_buckets()));
        }
        if let Some((_, buckets)) = current.as_mut()
            && let Some(bb) = buckets.iter_mut().find(|b| b.bucket == *bucket)
        {
            bb.listens = *listens;
        }
    }
    if let Some(prev) = current.take() {
        per_label.push(prev);
    }

    // Cap month rows at the most recent 24; cap day rows at 365.
    let cap = match granularity {
        Granularity::Day => RIDGELINE_DAY_CAP as usize,
        Granularity::Week => usize::MAX,
        Granularity::Month => MONTH_ROW_CAP,
    };
    if per_label.len() > cap {
        let skip = per_label.len() - cap;
        per_label = per_label.into_iter().skip(skip).collect();
    }

    let tempo_rows: Vec<TempoRow> = per_label
        .into_iter()
        .map(|(label, buckets)| TempoRow {
            label,
            granularity,
            buckets,
        })
        .collect();

    // Per-listen weighted stats. Same query, but the (bpm, listens) pairs aggregate
    // across the whole window so popular tracks dominate the median/mode/sigma.
    let mut weighted_stmt = conn.prepare(&format!(
        "SELECT adf.bpm, COUNT(*) AS listens
         FROM listen_history lh
         JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
           AND adf.bpm >= {min} AND adf.bpm < {max}
         GROUP BY adf.bpm",
        min = BPM_MIN,
        max = BPM_MAX
    ))?;
    let weighted: Vec<(f64, i64)> = weighted_stmt
        .query_map(params![days], |row| {
            Ok((row.get::<_, f64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let median = weighted_median_bpm(&weighted);
    let sigma = weighted_stddev_bpm(&weighted);
    // Mode = bucket centre (lower-edge + step/2) of the listens-argmax bucket.
    let mode = mode_bucket_centre(&tempo_rows);

    let stats = TempoStats {
        median,
        mode,
        sigma,
    };

    // Coverage: analysed tracks / total listened tracks within the window.
    let total_listened: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT track_id)
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))",
        params![days],
        |row| row.get(0),
    )?;
    let analyzed: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT lh.track_id)
         FROM listen_history lh
         JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
           AND adf.bpm IS NOT NULL",
        params![days],
        |row| row.get(0),
    )?;

    // ridge_amp_max = P95 across all per-row per-bucket density values.
    let mut all_listens: Vec<i64> = tempo_rows
        .iter()
        .flat_map(|r| r.buckets.iter().map(|b| b.listens))
        .collect();
    all_listens.sort_unstable();
    let ridge_amp_max = percentile_i64(&all_listens, 95.0).unwrap_or(0.0);

    Ok(TempoView {
        bucket_axis: BucketAxis {
            min: BPM_MIN,
            max: BPM_MAX,
            step: BPM_STEP,
        },
        rows: tempo_rows,
        stats,
        coverage: Coverage {
            analyzed,
            total_listened,
        },
        ridge_amp_max,
    })
}

fn dense_buckets() -> Vec<BpmBucket> {
    (0..BPM_BUCKET_COUNT)
        .map(|i| BpmBucket {
            bucket: BPM_MIN + (i as i32) * BPM_STEP,
            listens: 0,
        })
        .collect()
}

fn mode_bucket_centre(rows: &[TempoRow]) -> Option<f64> {
    let mut totals: HashMap<i32, i64> = HashMap::new();
    for r in rows {
        for b in &r.buckets {
            *totals.entry(b.bucket).or_insert(0) += b.listens;
        }
    }
    let (best_bucket, best_listens) = totals.into_iter().max_by_key(|(_, l)| *l)?;
    if best_listens == 0 {
        return None;
    }
    Some(best_bucket as f64 + (BPM_STEP as f64) / 2.0)
}

fn percentile_i64(sorted_asc: &[i64], pct: f64) -> Option<f64> {
    if sorted_asc.is_empty() {
        return None;
    }
    let n = sorted_asc.len();
    let rank = (pct / 100.0) * (n as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        return Some(sorted_asc[lo] as f64);
    }
    let frac = rank - lo as f64;
    Some(sorted_asc[lo] as f64 * (1.0 - frac) + sorted_asc[hi] as f64 * frac)
}

// --- Sonic field ------------------------------------------------------------

fn get_signals_sonic_field(conn: &Connection, days: i64) -> Result<SonicView> {
    let mut stmt = conn.prepare(
        "SELECT
            lh.track_id,
            t.title,
            ar.name AS artist_name,
            al.title AS album,
            al.artwork_url AS artwork_path,
            t.file_path,
            adf.energy,
            adf.danceability,
            adf.bpm,
            COUNT(*) AS listens
         FROM listen_history lh
         JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         JOIN tracks t ON t.id = lh.track_id
         LEFT JOIN artists ar ON ar.id = t.artist_id
         LEFT JOIN albums al ON al.id = t.album_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
           AND adf.energy IS NOT NULL
           AND adf.danceability IS NOT NULL
           AND adf.bpm IS NOT NULL
           AND adf.bpm >= ?2 AND adf.bpm < ?3
         GROUP BY lh.track_id, t.title, ar.name, al.title, al.artwork_url, t.file_path, adf.energy, adf.danceability, adf.bpm
         ORDER BY listens DESC, t.title ASC
         LIMIT ?4",
    )?;
    let tracks: Vec<SonicTrack> = stmt
        .query_map(
            params![days, BPM_MIN as f64, BPM_MAX as f64, SONIC_FIELD_LIMIT],
            |row| {
                Ok(SonicTrack {
                    track_id: row.get(0)?,
                    title: row.get(1)?,
                    artist_name: row.get(2)?,
                    album: row.get(3)?,
                    artwork_path: row.get(4)?,
                    file_path: row.get(5)?,
                    e: row.get(6)?,
                    d: row.get(7)?,
                    bpm: row.get(8)?,
                    listens: row.get(9)?,
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()?;

    let total_listened: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT track_id)
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))",
        params![days],
        |row| row.get(0),
    )?;
    let analyzed: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT lh.track_id)
         FROM listen_history lh
         JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
           AND adf.energy IS NOT NULL AND adf.danceability IS NOT NULL AND adf.bpm IS NOT NULL",
        params![days],
        |row| row.get(0),
    )?;

    let total = tracks.len() as i64;
    Ok(SonicView {
        tracks,
        total,
        coverage: Coverage {
            analyzed,
            total_listened,
        },
    })
}

// --- Ridgeline (hero) -------------------------------------------------------

fn get_signals_ridgeline(conn: &Connection, days: i64) -> Result<Vec<RidgeRow>> {
    // Cap at 365 days to keep the SVG sane; longer windows render the most-recent year.
    let effective = days.min(RIDGELINE_DAY_CAP);

    // Pull the per-day per-hour listens, then zero-fill the date axis so every day in the
    // window renders even if it has no listens. Per the plan: "one ridge per day in the
    // chosen window" - a flat row IS the ridge for an empty day.
    let mut stmt = conn.prepare(
        "SELECT
            DATE(started_at, 'localtime') AS day,
            CAST(strftime('%H', started_at, 'localtime') AS INTEGER) AS hour,
            COUNT(*) AS listens
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY day, hour
         ORDER BY day, hour",
    )?;
    let mut by_day_map: std::collections::BTreeMap<String, [i64; 24]> =
        std::collections::BTreeMap::new();
    let rows = stmt.query_map(params![effective], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i32>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for r in rows {
        let (day, hour, listens) = r?;
        let entry = by_day_map.entry(day).or_insert([0i64; 24]);
        if (0..24).contains(&hour) {
            entry[hour as usize] = listens;
        }
    }

    // Build the canonical date axis (oldest -> newest, inclusive of today).
    let axis_dates: Vec<String> = conn
        .prepare(
            "WITH RECURSIVE axis(d) AS (
                SELECT DATE(datetime('now', 'localtime', printf('-%d days', ?1 - 1)))
                UNION ALL
                SELECT DATE(d, '+1 day') FROM axis WHERE d < DATE('now', 'localtime')
            )
            SELECT d FROM axis",
        )?
        .query_map(params![effective], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;

    Ok(axis_dates
        .into_iter()
        .map(|date| {
            let hourly = by_day_map
                .get(&date)
                .copied()
                .unwrap_or([0i64; 24])
                .to_vec();
            RidgeRow { date, hourly }
        })
        .collect())
}

// --- Windowed top tracks / artists / genres ---------------------------------

fn get_top_tracks_windowed(
    conn: &Connection,
    days: i64,
    limit: i64,
    window_listened_ms: i64,
) -> Result<Vec<AnalyticsTopTrack>> {
    let previous_ranks = previous_track_ranks(conn, days)?;
    let sql = format!(
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url,
                COUNT(lh.id) AS listens,
                COALESCE(SUM(CASE WHEN lh.completed = 1 THEN 1 ELSE 0 END), 0) AS completed_listens,
                COALESCE(SUM({capped}), 0) AS total_listened_ms
         FROM listen_history lh
         JOIN tracks t ON lh.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
           AND {CHOSEN_LISTENS_ONLY}
         GROUP BY t.id, t.title, a.name, al.title, al.artwork_url
         ORDER BY listens DESC, total_listened_ms DESC, t.title ASC
         LIMIT ?2",
        capped = capped_listened_ms("lh", "t")
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map(params![days, limit], |row| {
            Ok(AnalyticsTopTrack {
                track_id: row.get(0)?,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                artwork_url: row.get(4)?,
                listens: row.get(5)?,
                completed_listens: row.get(6)?,
                total_listened_ms: row.get(7)?,
                completion_rate: None,
                share_of_window_listened_ms: None,
                previous_rank: None,
                rank_delta: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (idx, row) in rows.iter_mut().enumerate() {
        let current_rank = idx as i64 + 1;
        row.completion_rate = ratio_or_none(row.completed_listens, row.listens);
        row.share_of_window_listened_ms = ratio_or_none(row.total_listened_ms, window_listened_ms);
        row.previous_rank = previous_ranks.get(&row.track_id).copied();
        row.rank_delta = row
            .previous_rank
            .map(|previous_rank| previous_rank - current_rank);
    }
    Ok(rows)
}

fn get_top_artists_windowed(
    conn: &Connection,
    days: i64,
    limit: i64,
    window_listened_ms: i64,
) -> Result<Vec<AnalyticsTopArtist>> {
    let previous_ranks = previous_artist_ranks(conn, days)?;
    // Ranked by listened time because that is the metric the analytics card
    // displays; raw listen counts include instant skip-starts and produced
    // orderings that contradicted the times shown next to them.
    // `previous_artist_ranks` must sort by the same key.
    let sql = format!(
        "SELECT a.id, a.name,
                COUNT(lh.id) AS listens,
                COALESCE(SUM(CASE WHEN lh.completed = 1 THEN 1 ELSE 0 END), 0) AS completed_listens,
                COUNT(DISTINCT t.id) AS unique_tracks,
                COALESCE(SUM({capped}), 0) AS total_listened_ms
         FROM listen_history lh
         JOIN tracks t ON lh.track_id = t.id
         JOIN artists a ON t.artist_id = a.id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
           AND {CHOSEN_LISTENS_ONLY}
         GROUP BY a.id, a.name
         ORDER BY total_listened_ms DESC, listens DESC, a.name ASC
         LIMIT ?2",
        capped = capped_listened_ms("lh", "t")
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map(params![days, limit], |row| {
            Ok(AnalyticsTopArtist {
                artist_id: row.get(0)?,
                artist_name: row.get(1)?,
                listens: row.get(2)?,
                completed_listens: row.get(3)?,
                unique_tracks: row.get(4)?,
                total_listened_ms: row.get(5)?,
                completion_rate: None,
                share_of_window_listened_ms: None,
                previous_rank: None,
                rank_delta: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (idx, row) in rows.iter_mut().enumerate() {
        let current_rank = idx as i64 + 1;
        row.completion_rate = ratio_or_none(row.completed_listens, row.listens);
        row.share_of_window_listened_ms = ratio_or_none(row.total_listened_ms, window_listened_ms);
        row.previous_rank = previous_ranks.get(&row.artist_id).copied();
        row.rank_delta = row
            .previous_rank
            .map(|previous_rank| previous_rank - current_rank);
    }
    Ok(rows)
}

fn get_top_genres_windowed(
    conn: &Connection,
    days: i64,
    limit: i64,
    window_listens: i64,
) -> Result<Vec<AnalyticsGenreShare>> {
    let mut stmt = conn.prepare(
        "SELECT g.name, COUNT(lh.id) AS listens
         FROM listen_history lh
         JOIN track_genres tg ON lh.track_id = tg.track_id
         JOIN genres g ON tg.genre_id = g.id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY g.id, g.name
         ORDER BY listens DESC, g.name ASC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![days, limit], |row| {
            Ok(AnalyticsGenreShare {
                genre_name: row.get(0)?,
                listens: row.get(1)?,
                share_of_window_listens: ratio_or_none(row.get(1)?, window_listens),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn previous_track_ranks(conn: &Connection, days: i64) -> Result<HashMap<i64, i64>> {
    let sql = format!(
        "SELECT track_id, rank FROM (
            SELECT
                t.id AS track_id,
                ROW_NUMBER() OVER (
                    ORDER BY COUNT(lh.id) DESC,
                             COALESCE(SUM({capped}), 0) DESC,
                             t.title ASC
                ) AS rank
            FROM listen_history lh
            JOIN tracks t ON lh.track_id = t.id
            WHERE lh.started_at >= datetime('now', printf('-%d days', ?1 * 2))
              AND lh.started_at < datetime('now', printf('-%d days', ?1))
              AND {CHOSEN_LISTENS_ONLY}
            GROUP BY t.id, t.title
        )",
        capped = capped_listened_ms("lh", "t")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![days], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().collect())
}

fn previous_artist_ranks(conn: &Connection, days: i64) -> Result<HashMap<i64, i64>> {
    let sql = format!(
        "SELECT artist_id, rank FROM (
            SELECT
                a.id AS artist_id,
                ROW_NUMBER() OVER (
                    ORDER BY COALESCE(SUM({capped}), 0) DESC,
                             COUNT(lh.id) DESC,
                             a.name ASC
                ) AS rank
            FROM listen_history lh
            JOIN tracks t ON lh.track_id = t.id
            JOIN artists a ON t.artist_id = a.id
            WHERE lh.started_at >= datetime('now', printf('-%d days', ?1 * 2))
              AND lh.started_at < datetime('now', printf('-%d days', ?1))
              AND {CHOSEN_LISTENS_ONLY}
            GROUP BY a.id, a.name
        )",
        capped = capped_listened_ms("lh", "t")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![days], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().collect())
}

// --- Cohorts ----------------------------------------------------------------

fn get_signals_cohorts(conn: &Connection, days: i64) -> Result<Vec<Cohort>> {
    // Per-track first_at + lifetime_listens via the new idx_listen_history_track_started index.
    let sql = format!("
        WITH first_listens AS (
            SELECT track_id,
                   MIN(started_at) AS first_at,
                   COUNT(*) AS lifetime_listens
            FROM listen_history
            GROUP BY track_id
        ),
        windowed AS (
            SELECT lh.id, lh.track_id, lh.duration_listened_ms, lh.completed, lh.session_id
            FROM listen_history lh
            WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
        ),
        joined AS (
            SELECT
                w.id, w.track_id, {capped} AS duration_listened_ms, w.completed, w.session_id,
                fl.first_at, fl.lifetime_listens,
                t.artist_id,
                CASE
                    WHEN fl.first_at >= datetime('now', printf('-%d days', ?1)) THEN 'new_this_month'
                    WHEN fl.first_at < datetime('now', printf('-%d days', ?2))
                         AND fl.lifetime_listens >= ?3 THEN 'deep_cuts'
                    ELSE 'established'
                END AS cohort_key
            FROM windowed w
            JOIN first_listens fl ON fl.track_id = w.track_id
            JOIN tracks t ON t.id = w.track_id
        )
        SELECT
            cohort_key,
            COUNT(DISTINCT track_id) AS tracks,
            COALESCE(SUM(duration_listened_ms), 0) AS listened_ms,
            COUNT(DISTINCT CASE WHEN session_id IS NOT NULL THEN session_id END) AS sessions,
            COUNT(*) AS listens,
            COALESCE(SUM(CASE WHEN completed = 1 THEN 1 ELSE 0 END), 0) AS completed_listens,
            COUNT(DISTINCT CASE WHEN first_at >= datetime('now', printf('-%d days', ?1)) THEN artist_id END) AS new_artists
        FROM joined
        GROUP BY cohort_key
    ", capped = capped_listened_ms("w", "t"));
    let mut stmt = conn.prepare(&sql)?;
    let rows: HashMap<String, (i64, i64, i64, i64, i64, i64)> = stmt
        .query_map(
            params![days, COHORT_DEEP_DAYS, COHORT_DEEP_LIFETIME_LISTENS],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (
                        row.get::<_, i64>(1)?, // tracks
                        row.get::<_, i64>(2)?, // listened_ms
                        row.get::<_, i64>(3)?, // sessions
                        row.get::<_, i64>(4)?, // listens
                        row.get::<_, i64>(5)?, // completed_listens
                        row.get::<_, i64>(6)?, // new_artists
                    ),
                ))
            },
        )?
        .collect::<rusqlite::Result<_>>()?;

    let _ = COHORT_NEW_DAYS; // current cohort window matches `days`; reserved for future split.

    let labels = [
        ("new_this_month", "New in selected window"),
        ("established", "Established"),
        ("deep_cuts", "Deep cuts"),
    ];
    let mut out: Vec<Cohort> = Vec::with_capacity(3);
    for (key, label) in labels {
        let (tracks, listened_ms, sessions, listens, completed, new_artists) =
            rows.get(key).copied().unwrap_or((0, 0, 0, 0, 0, 0));
        let completion = ratio_or_none(completed, listens);
        let skip_rate = completion.map(|c| 1.0 - c);
        let repeat_rate = if tracks == 0 {
            None
        } else {
            Some(listens as f64 / tracks as f64)
        };
        out.push(Cohort {
            key: key.to_string(),
            label: label.to_string(),
            tracks,
            listened_ms,
            sessions,
            completion,
            skip_rate,
            new_artists,
            repeat_rate,
        });
    }
    Ok(out)
}

// --- Audio profile ----------------------------------------------------------

fn get_signals_audio_profile(conn: &Connection, days: i64) -> Result<AudioProfile> {
    // Listen-weighted loudness vector + spectral centroid mean.
    let mut stmt = conn.prepare(
        "SELECT adf.loudness_lufs, adf.spectral_centroid
         FROM listen_history lh
         JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))",
    )?;
    let pairs: Vec<(Option<f64>, Option<f64>)> = stmt
        .query_map(params![days], |row| {
            Ok((row.get::<_, Option<f64>>(0)?, row.get::<_, Option<f64>>(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let loudness_vals: Vec<f64> = pairs.iter().filter_map(|(l, _)| *l).collect();
    let centroid_vals: Vec<f64> = pairs.iter().filter_map(|(_, c)| *c).collect();

    let (track_total, track_analyzed, listen_total, listen_analyzed): (i64, i64, i64, i64) = conn
        .query_row(
        "SELECT
            COUNT(DISTINCT lh.track_id),
            COUNT(DISTINCT CASE
                WHEN adf.loudness_lufs IS NOT NULL OR adf.spectral_centroid IS NOT NULL
                THEN lh.track_id
            END),
            COUNT(lh.id),
            COUNT(CASE
                WHEN adf.loudness_lufs IS NOT NULL OR adf.spectral_centroid IS NOT NULL
                THEN 1
            END)
         FROM listen_history lh
         LEFT JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))",
        params![days],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;

    let loudness_lufs = if loudness_vals.is_empty() {
        None
    } else {
        Some(loudness_vals.iter().sum::<f64>() / loudness_vals.len() as f64)
    };

    let dynamic_range_dr = if loudness_vals.len() < 5 {
        None
    } else {
        let mut sorted = loudness_vals.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p5 = percentile_f64(&sorted, 5.0);
        let p95 = percentile_f64(&sorted, 95.0);
        match (p5, p95) {
            (Some(lo), Some(hi)) => Some((hi - lo).max(0.0)),
            _ => None,
        }
    };

    let bass_tilt = if centroid_vals.is_empty() {
        None
    } else {
        let mean = centroid_vals.iter().sum::<f64>() / centroid_vals.len() as f64;
        if mean <= 0.0 {
            None
        } else {
            // bass_tilt = clamp(20 * log10(2000 / mean_centroid), -6, +6)
            Some((20.0 * (2000.0_f64 / mean).log10()).clamp(-6.0, 6.0))
        }
    };
    let treble_tilt = bass_tilt.map(|b| -b);

    Ok(AudioProfile {
        dynamic_range_dr,
        loudness_lufs,
        bass_tilt,
        treble_tilt,
        coverage: Coverage {
            analyzed: track_analyzed,
            total_listened: track_total,
        },
        track_coverage: Coverage {
            analyzed: track_analyzed,
            total_listened: track_total,
        },
        listen_coverage: Coverage {
            analyzed: listen_analyzed,
            total_listened: listen_total,
        },
    })
}

fn percentile_f64(sorted_asc: &[f64], pct: f64) -> Option<f64> {
    if sorted_asc.is_empty() {
        return None;
    }
    let n = sorted_asc.len();
    let rank = (pct / 100.0) * (n as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        return Some(sorted_asc[lo]);
    }
    let frac = rank - lo as f64;
    Some(sorted_asc[lo] * (1.0 - frac) + sorted_asc[hi] * frac)
}

// --- Top-level signals fetcher ----------------------------------------------

pub struct Signals;

impl Signals {
    pub fn compute(conn: &Connection, days: i64) -> Result<AnalyticsSignals> {
        get_analytics_signals(conn, days)
    }
}

fn get_analytics_signals(conn: &Connection, days: i64) -> Result<AnalyticsSignals> {
    let granularity = select_granularity(conn, days)?;
    let totals = get_analytics_totals(conn, days)?;
    let kpis = get_signals_kpis(conn, days)?;
    let tempo = get_signals_tempo(conn, days, granularity)?;
    let sonic_field = get_signals_sonic_field(conn, days)?;
    let ridgeline = get_signals_ridgeline(conn, days)?;
    let top_tracks = get_top_tracks_windowed(conn, days, 5, totals.listened_ms)?;
    let top_artists = get_top_artists_windowed(conn, days, 5, totals.listened_ms)?;
    let top_genres = get_top_genres_windowed(conn, days, 6, totals.listens)?;
    let cohorts = get_signals_cohorts(conn, days)?;
    let audio_profile = get_signals_audio_profile(conn, days)?;
    let window = build_signals_window(conn, days, granularity)?;

    Ok(AnalyticsSignals {
        window,
        totals,
        kpis,
        tempo,
        sonic_field,
        ridgeline,
        top_tracks,
        top_artists,
        top_genres,
        cohorts,
        audio_profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::Connection;

    // --- Analytics signals tests ------------------------------------------

    /// Rhythm: even routine = high score (low CV -> rhythm near 100).
    #[test]
    fn compute_rhythm_even_routine_scores_high() {
        let mut days = Vec::new();
        for _ in 0..7 {
            // 1 listen each hour, every hour, every day = perfect routine.
            days.push([1i64; 24]);
        }
        let r = compute_rhythm(&days).expect("active days >= 5");
        assert!(r >= 95, "even routine should score near 100, got {r}");
    }

    /// Rhythm: spiky one-day pattern = low score (high CV -> rhythm near 0).
    #[test]
    fn compute_rhythm_spiky_scores_low() {
        let mut days = Vec::new();
        for _ in 0..7 {
            // All 24 listens in hour 21, zero everywhere else.
            let mut h = [0i64; 24];
            h[21] = 24;
            days.push(h);
        }
        let r = compute_rhythm(&days).expect("active days >= 5");
        assert!(r <= 5, "spiky pattern should score near 0, got {r}");
    }

    /// Rhythm: <5 active days returns None (renders as `--` in the UI).
    #[test]
    fn compute_rhythm_returns_none_below_floor() {
        let days = vec![[1i64; 24]; 4];
        assert!(compute_rhythm(&days).is_none());
    }

    /// Rhythm: zero-listen days are excluded from the active count.
    #[test]
    fn compute_rhythm_ignores_empty_days() {
        let mut days = vec![[0i64; 24]; 30];
        for d in days.iter_mut().take(4) {
            d[12] = 5;
        }
        // 4 active days < 5 floor -> None.
        assert!(compute_rhythm(&days).is_none());
    }

    /// Listen-weighted median: the canonical fixture from the plan -
    /// 200 plays of a 124 BPM track and 5 plays each of 10 other tracks
    /// MUST produce a median near 124, not the per-track median (~80-something).
    #[test]
    fn weighted_median_is_listen_weighted_not_per_track() {
        // 200 plays of one popular track at 124 BPM, 5 plays each of 10 tracks at
        // unrelated BPMs spanning the rest of the range.
        let mut weighted = vec![(124.0_f64, 200_i64)];
        for bpm in [
            62.0, 70.0, 78.0, 86.0, 94.0, 100.0, 108.0, 142.0, 160.0, 180.0,
        ]
        .iter()
        {
            weighted.push((*bpm, 5));
        }
        let med = weighted_median_bpm(&weighted).expect("non-empty");
        assert!(
            (med - 124.0).abs() < 0.01,
            "expected listen-weighted median near 124, got {med}"
        );
    }

    /// Listen-weighted stddev across the same per-listen vector.
    #[test]
    fn weighted_stddev_reflects_listen_weights() {
        // Two BPMs, equal weights -> stddev should equal half the spread.
        let pairs = [(100.0, 5_i64), (140.0, 5_i64)];
        let s = weighted_stddev_bpm(&pairs).expect("non-empty");
        assert!((s - 20.0).abs() < 0.01, "expected ~20, got {s}");
    }

    /// Mode bucket centre: argmax bucket -> returns lower-edge + step/2.
    #[test]
    fn mode_bucket_returns_centre() {
        let row = TempoRow {
            label: "row".to_string(),
            granularity: Granularity::Day,
            buckets: dense_buckets(),
        };
        let mut rows = vec![row.clone(), row];
        // Bump bucket 124 in the first row, bucket 100 in the second - total argmax = 124.
        rows[0]
            .buckets
            .iter_mut()
            .find(|b| b.bucket == 124)
            .unwrap()
            .listens = 50;
        rows[1]
            .buckets
            .iter_mut()
            .find(|b| b.bucket == 100)
            .unwrap()
            .listens = 30;
        let mode = mode_bucket_centre(&rows).expect("non-empty");
        // 124 + step/2 = 124 + 2 = 126.
        assert!((mode - 126.0).abs() < 0.01, "expected 126.0, got {mode}");
    }

    /// Granularity selection: short windows always pick Day.
    #[test]
    fn granularity_short_windows_pick_day() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        for d in [1, 7] {
            assert_eq!(select_granularity(&conn, d).expect("ok"), Granularity::Day);
        }
    }

    /// Granularity selection: 90d picks Week.
    #[test]
    fn granularity_90d_picks_week() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        assert_eq!(
            select_granularity(&conn, 90).expect("ok"),
            Granularity::Week
        );
    }

    /// Granularity selection: very long windows pick Month.
    #[test]
    fn granularity_all_picks_month() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        assert_eq!(
            select_granularity(&conn, 36500).expect("ok"),
            Granularity::Month
        );
    }

    /// Granularity selection: 30d with sparse data falls back to Week.
    /// Empty DB -> distinct_days = 0 < 15 -> Week.
    #[test]
    fn granularity_30d_sparse_falls_back_to_week() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        // No listen_history rows -> sparse -> Week fallback.
        assert_eq!(
            select_granularity(&conn, 30).expect("ok"),
            Granularity::Week
        );
    }

    /// Dense buckets cover the full BPM axis at step granularity, in order.
    #[test]
    fn dense_buckets_match_axis() {
        let buckets = dense_buckets();
        assert_eq!(buckets.len(), BPM_BUCKET_COUNT);
        assert_eq!(buckets.first().unwrap().bucket, BPM_MIN);
        assert_eq!(
            buckets.last().unwrap().bucket,
            BPM_MIN + ((BPM_BUCKET_COUNT - 1) as i32) * BPM_STEP
        );
        for w in buckets.windows(2) {
            assert_eq!(w[1].bucket - w[0].bucket, BPM_STEP);
        }
    }

    /// Empty signals on a fresh DB return zero everything, no panics, valid shape.
    #[test]
    fn analytics_signals_empty_db_returns_valid_shape() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        let s = Signals::compute(&conn, 30).expect("signals");
        assert_eq!(s.kpis.listened_ms.current, 0);
        assert_eq!(s.kpis.sessions.current, 0);
        assert!(s.kpis.completion.current.is_none());
        assert_eq!(s.tempo.bucket_axis.min, BPM_MIN);
        assert_eq!(s.tempo.bucket_axis.max, BPM_MAX);
        assert_eq!(s.tempo.bucket_axis.step, BPM_STEP);
        assert_eq!(s.sonic_field.total, 0);
        assert_eq!(s.ridgeline.len(), 30);
        assert!(
            s.ridgeline
                .iter()
                .all(|row| row.hourly.len() == 24 && row.hourly.iter().all(|count| *count == 0))
        );
        assert_eq!(s.cohorts.len(), 3);
        assert!(s.audio_profile.loudness_lufs.is_none());
    }

    fn seed_analytics_track(
        conn: &Connection,
        track_id: i64,
        artist_id: i64,
        title: &str,
        artist: &str,
    ) {
        conn.execute(
            "INSERT OR IGNORE INTO artists (id, name) VALUES (?1, ?2)",
            params![artist_id, artist],
        )
        .expect("seed artist");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms, source)
             VALUES (?1, ?2, ?3, 180000, 'tidal')",
            params![track_id, title, artist_id],
        )
        .expect("seed track");
    }

    fn seed_analytics_listen(
        conn: &Connection,
        id: i64,
        track_id: i64,
        date_modifier: &str,
        duration_ms: i64,
        completed: bool,
        session_id: Option<&str>,
    ) {
        seed_analytics_listen_src(
            conn,
            id,
            track_id,
            date_modifier,
            duration_ms,
            completed,
            session_id,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn seed_analytics_listen_src(
        conn: &Connection,
        id: i64,
        track_id: i64,
        date_modifier: &str,
        duration_ms: i64,
        completed: bool,
        session_id: Option<&str>,
        source: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO listen_history
                (id, track_id, started_at, duration_listened_ms, completed, session_id, source)
             VALUES (?1, ?2, datetime('now', ?3), ?4, ?5, ?6, ?7)",
            params![
                id,
                track_id,
                date_modifier,
                duration_ms,
                completed as i32,
                session_id,
                source
            ],
        )
        .expect("seed listen");
    }

    fn sqlite_local_date(conn: &Connection, date_modifier: &str) -> String {
        conn.query_row(
            "SELECT DATE(datetime('now', 'localtime', ?1))",
            params![date_modifier],
            |row| row.get(0),
        )
        .expect("date")
    }

    #[test]
    fn analytics_signals_dense_daily_rows_include_sessions() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_analytics_track(&conn, 1, 1, "Pulse One", "NOOR Artist");
        seed_analytics_listen(&conn, 1, 1, "-2 days", 100_000, true, Some("s-a"));
        seed_analytics_listen(&conn, 2, 1, "-2 days", 80_000, false, Some("s-a"));
        seed_analytics_listen(&conn, 3, 1, "-0 days", 120_000, true, Some("s-b"));

        let s = Signals::compute(&conn, 3).expect("signals");
        assert_eq!(s.kpis.daily.len(), 3);
        assert_eq!(s.kpis.sessions.current, 2);

        let first_day = sqlite_local_date(&conn, "-2 days");
        let middle_day = sqlite_local_date(&conn, "-1 days");
        let today = sqlite_local_date(&conn, "-0 days");
        let first = s
            .kpis
            .daily
            .iter()
            .find(|row| row.day == first_day)
            .expect("first active day");
        let middle = s
            .kpis
            .daily
            .iter()
            .find(|row| row.day == middle_day)
            .expect("inactive day");
        let last = s
            .kpis
            .daily
            .iter()
            .find(|row| row.day == today)
            .expect("today");

        assert_eq!(first.listens, 2);
        assert_eq!(first.sessions, 1);
        assert_eq!(middle.listens, 0);
        assert_eq!(middle.sessions, 0);
        assert_eq!(last.listens, 1);
        assert_eq!(last.sessions, 1);
    }

    #[test]
    fn analytics_signals_window_metadata_uses_real_iso_timestamps_and_caps() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        let s = Signals::compute(&conn, 30).expect("signals");
        assert!(s.window.started_at.contains('T'));
        assert!(s.window.previous_started_at.contains('T'));
        assert!(s.window.generated_at.contains('T'));
        assert!(s.window.started_at.ends_with('Z'));
        assert!(!s.window.started_at.contains("datetime("));
        assert_eq!(s.window.granularity, Granularity::Week);
        assert_eq!(s.window.display_caps.ridgeline_days, None);
        assert_eq!(s.totals.listens, 0);
        assert_eq!(s.totals.listened_ms, 0);
        assert_eq!(s.totals.distinct_tracks, 0);
        assert_eq!(s.totals.tagged_listens, 0);

        let long = Signals::compute(&conn, 36500).expect("long signals");
        assert_eq!(long.window.granularity, Granularity::Month);
        assert_eq!(
            long.window.display_caps.ridgeline_days,
            Some(RIDGELINE_DAY_CAP)
        );
    }

    #[test]
    fn analytics_signals_genre_share_and_rank_metrics_use_window_denominators() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_analytics_track(&conn, 1, 1, "Tagged", "Tagged Artist");
        seed_analytics_track(&conn, 2, 2, "Popular", "Popular Artist");
        conn.execute(
            "INSERT INTO genres (id, name, slug) VALUES (1, 'Ambient', 'ambient')",
            [],
        )
        .expect("seed genre");
        conn.execute(
            "INSERT INTO track_genres (track_id, genre_id, source, confidence)
             VALUES (1, 1, 'lastfm', 1.0)",
            [],
        )
        .expect("seed track genre");

        // Durations stay below the seeded 180 s track length so the stall cap
        // does not engage; this test targets the share denominators.
        seed_analytics_listen(&conn, 1, 1, "-2 days", 100_000, true, Some("s-a"));
        seed_analytics_listen(&conn, 2, 2, "-1 days", 100_000, true, Some("s-b"));
        seed_analytics_listen(&conn, 3, 2, "-1 days", 100_000, true, Some("s-b"));
        seed_analytics_listen(&conn, 4, 2, "-0 days", 100_000, false, Some("s-c"));
        seed_analytics_listen(&conn, 5, 1, "-4 days", 100_000, true, Some("p-a"));
        seed_analytics_listen(&conn, 6, 1, "-4 days", 100_000, true, Some("p-b"));
        seed_analytics_listen(&conn, 7, 2, "-4 days", 100_000, true, Some("p-c"));

        let s = Signals::compute(&conn, 3).expect("signals");
        assert_eq!(s.totals.listens, 4);
        assert_eq!(s.totals.listened_ms, 400_000);
        assert_eq!(s.totals.distinct_tracks, 2);
        assert_eq!(s.totals.tagged_listens, 1);

        let genre = s.top_genres.first().expect("genre row");
        assert_eq!(genre.genre_name, "Ambient");
        assert_eq!(genre.listens, 1);
        assert_eq!(genre.share_of_window_listens, Some(0.25));

        let popular_track = s
            .top_tracks
            .iter()
            .find(|track| track.track_id == 2)
            .expect("popular track");
        assert_eq!(popular_track.completion_rate, Some(2.0 / 3.0));
        assert_eq!(popular_track.share_of_window_listened_ms, Some(0.75));
        assert_eq!(popular_track.previous_rank, Some(2));
        assert_eq!(popular_track.rank_delta, Some(1));

        let popular_artist = s
            .top_artists
            .iter()
            .find(|artist| artist.artist_id == 2)
            .expect("popular artist");
        assert_eq!(popular_artist.completion_rate, Some(2.0 / 3.0));
        assert_eq!(popular_artist.share_of_window_listened_ms, Some(0.75));
        assert_eq!(popular_artist.previous_rank, Some(2));
        assert_eq!(popular_artist.rank_delta, Some(1));
    }

    /// Top artists rank by listened time (the metric the card displays), not
    /// by raw listen-event count; previous-window ranks use the same key so
    /// rank deltas compare like-for-like.
    #[test]
    fn analytics_signals_top_artists_rank_by_listened_time() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_analytics_track(&conn, 11, 10, "Short Loops", "Sprinter");
        seed_analytics_track(&conn, 21, 20, "Long Cut", "Marathoner");

        // Current window: Sprinter has more listen events, Marathoner more
        // time. Durations stay below the 180 s track length so the stall cap
        // does not engage; this test targets the ordering.
        seed_analytics_listen(&conn, 1, 11, "-1 days", 40_000, false, Some("s-a"));
        seed_analytics_listen(&conn, 2, 11, "-1 days", 40_000, false, Some("s-a"));
        seed_analytics_listen(&conn, 3, 11, "-1 days", 40_000, false, Some("s-a"));
        seed_analytics_listen(&conn, 4, 21, "-1 days", 160_000, true, Some("s-a"));
        // Previous window: flipped, Sprinter had more time.
        seed_analytics_listen(&conn, 5, 11, "-4 days", 60_000, true, Some("p-a"));
        seed_analytics_listen(&conn, 6, 11, "-4 days", 60_000, true, Some("p-a"));
        seed_analytics_listen(&conn, 7, 21, "-4 days", 50_000, true, Some("p-a"));

        let s = Signals::compute(&conn, 3).expect("signals");
        let names: Vec<&str> = s
            .top_artists
            .iter()
            .map(|artist| artist.artist_name.as_str())
            .collect();
        assert_eq!(names, vec!["Marathoner", "Sprinter"]);

        let marathoner = &s.top_artists[0];
        assert_eq!(marathoner.listens, 1);
        assert_eq!(marathoner.total_listened_ms, 160_000);
        assert_eq!(marathoner.previous_rank, Some(2));
        assert_eq!(marathoner.rank_delta, Some(1));

        let sprinter = &s.top_artists[1];
        assert_eq!(sprinter.previous_rank, Some(1));
        assert_eq!(sprinter.rank_delta, Some(-1));
    }

    /// Radio and automix pick tracks by themselves, so they are excluded from
    /// the taste-ranked cards; the KPI tiles keep counting every source.
    /// NULL-source legacy rows stay in the cards (provenance unknowable).
    #[test]
    fn analytics_signals_rank_cards_exclude_machine_picked_sources() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_analytics_track(&conn, 1, 1, "Chosen Cut", "Chosen Artist");
        seed_analytics_track(&conn, 2, 2, "Radio Filler", "Radio Artist");
        seed_analytics_track(&conn, 3, 3, "Automix Filler", "Automix Artist");
        seed_analytics_track(&conn, 4, 4, "Legacy Row", "Legacy Artist");

        seed_analytics_listen_src(
            &conn,
            1,
            1,
            "-1 days",
            60_000,
            true,
            Some("s"),
            Some("manual"),
        );
        // Radio outweighs the chosen listen 3x - it must still not rank.
        seed_analytics_listen_src(
            &conn,
            2,
            2,
            "-1 days",
            90_000,
            true,
            Some("s"),
            Some("radio"),
        );
        seed_analytics_listen_src(
            &conn,
            3,
            2,
            "-1 days",
            90_000,
            true,
            Some("s"),
            Some("radio"),
        );
        seed_analytics_listen_src(
            &conn,
            4,
            3,
            "-1 days",
            90_000,
            true,
            Some("s"),
            Some("automix"),
        );
        seed_analytics_listen_src(&conn, 5, 4, "-1 days", 30_000, true, Some("s"), None);

        let s = Signals::compute(&conn, 3).expect("signals");

        let artist_names: Vec<&str> = s
            .top_artists
            .iter()
            .map(|artist| artist.artist_name.as_str())
            .collect();
        assert_eq!(artist_names, vec!["Chosen Artist", "Legacy Artist"]);
        let track_titles: Vec<&str> = s
            .top_tracks
            .iter()
            .map(|track| track.title.as_str())
            .collect();
        assert_eq!(track_titles, vec!["Chosen Cut", "Legacy Row"]);

        // Tiles and totals still count all sources.
        assert_eq!(s.totals.listens, 5);
        assert_eq!(s.totals.listened_ms, 360_000);
        assert_eq!(s.kpis.listened_ms.current, 360_000);
    }

    /// A stalled player can record far more listened time than the track is
    /// long (observed: 2795 s on a 334 s track). Every listened-time surface
    /// on the page caps such rows at the track duration.
    #[test]
    fn analytics_signals_cap_runaway_listen_durations_at_track_length() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        // seed_analytics_track sets duration_ms = 180_000.
        seed_analytics_track(&conn, 1, 1, "Stalled", "Stall Artist");
        seed_analytics_listen(&conn, 1, 1, "-1 days", 2_795_000, true, Some("s-a"));
        seed_analytics_listen(&conn, 2, 1, "-1 days", 60_000, true, Some("s-a"));

        let s = Signals::compute(&conn, 3).expect("signals");
        assert_eq!(s.totals.listened_ms, 240_000);
        assert_eq!(s.kpis.listened_ms.current, 240_000);

        let track = s.top_tracks.first().expect("track row");
        assert_eq!(track.total_listened_ms, 240_000);
        let artist = s.top_artists.first().expect("artist row");
        assert_eq!(artist.total_listened_ms, 240_000);

        let new_cohort = s
            .cohorts
            .iter()
            .find(|cohort| cohort.key == "new_this_month")
            .expect("new cohort");
        assert_eq!(new_cohort.listened_ms, 240_000);
    }

    #[test]
    fn analytics_signals_audio_profile_coverage_matches_track_and_listen_totals() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_analytics_track(&conn, 1, 1, "Analysed", "Audio Artist");
        seed_analytics_track(&conn, 2, 1, "Missing DSP", "Audio Artist");
        conn.execute(
            "INSERT INTO audio_dsp_features
                (track_id, loudness_lufs, spectral_centroid, bpm, energy, danceability)
             VALUES (1, -12.0, 1800.0, 120.0, 0.6, 0.5)",
            [],
        )
        .expect("seed dsp");
        seed_analytics_listen(&conn, 1, 1, "-1 days", 120_000, true, Some("s-a"));
        seed_analytics_listen(&conn, 2, 1, "-1 days", 100_000, true, Some("s-a"));
        seed_analytics_listen(&conn, 3, 2, "-0 days", 90_000, false, Some("s-b"));

        let s = Signals::compute(&conn, 3).expect("signals");
        assert_eq!(s.audio_profile.coverage.analyzed, 1);
        assert_eq!(s.audio_profile.coverage.total_listened, 2);
        assert_eq!(s.audio_profile.track_coverage.analyzed, 1);
        assert_eq!(s.audio_profile.track_coverage.total_listened, 2);
        assert_eq!(s.audio_profile.listen_coverage.analyzed, 2);
        assert_eq!(s.audio_profile.listen_coverage.total_listened, 3);
        assert!(s.audio_profile.coverage.analyzed <= s.audio_profile.coverage.total_listened);
    }

    #[test]
    fn analytics_signals_cohort_label_matches_selected_window() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        let s = Signals::compute(&conn, 30).expect("signals");
        let new = s
            .cohorts
            .iter()
            .find(|cohort| cohort.key == "new_this_month")
            .expect("new cohort");
        assert_eq!(new.label, "New in selected window");
    }
}
