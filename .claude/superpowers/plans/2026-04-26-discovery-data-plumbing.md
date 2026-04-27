# Discovery Phase 1 — Data Plumbing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface ~12 hidden backend fields (cohort labels, edge reason tags, skip rates, audio DSP details, training freshness) on the `/api/discovery/space` payload and frontend types so Phase 2 visual work can render them — without changing any visuals yet.

**Architecture:** The `get_discovery_space` handler in `routes.rs` already builds an inline JSON response. We extend (a) the local `SpaceTrack` struct with new fields, (b) the existing SQL joins to populate them, (c) a new cohort-assignment helper, (d) the edge JSON to include `reason_tags` and the three score components (already pulled from DB but discarded during edge-type inference), and (e) a new `/api/discovery/space/meta` endpoint reading from `embedding_models` + `track_embeddings`. Frontend mirrors the new fields as optional properties on `DiscoverTrackNode` / `DiscoverEdge` and adds a dev-only `console.log` to validate the wire shape. No visual rendering changes in this phase.

**Tech Stack:** Rust (axum, rusqlite, serde_json), Svelte 5 + TypeScript. Type-check backend with `cd noor-server && cargo check`. Type-check frontend with `cd frontend && npx svelte-check --tsconfig ./tsconfig.json`. Smoke-test routes with `curl`.

> **Commit style for this repo:** Per repo convention, **do NOT add any `Co-Authored-By` trailer** to commits.

---

### Task 1: Extend `SpaceTrack` struct + DSP join with new audio fields

**Files:**
- Modify: `noor-server/src/server/routes.rs:1872-1887` (struct definition)
- Modify: `noor-server/src/server/routes.rs:2010-2053` (DSP fetch block)
- Modify: `noor-server/src/server/routes.rs:1932-1957` (radio path mapping)
- Modify: `noor-server/src/server/routes.rs:1985-2008` (fallback path mapping)
- Modify: `noor-server/src/server/routes.rs:1916-1931` (prompt path mapping)
- Modify: `noor-server/src/server/routes.rs:2156-2178` (node JSON output)

- [ ] **Step 1: Extend `SpaceTrack` struct**

In `routes.rs`, find the local `struct SpaceTrack` at line 1872. Replace it with:

```rust
#[derive(Debug)]
struct SpaceTrack {
    track_id: i64,
    title: String,
    artist_name: String,
    album_title: Option<String>,
    artwork_url: Option<String>,
    duration_ms: Option<i64>,
    similarity_score: f64,
    source: String,
    energy: Option<f64>,
    danceability: Option<f64>,
    bpm: Option<f64>,
    key_signature: Option<String>,
    camelot_key: Option<String>,
    is_instrumental: Option<bool>,
    loudness_lufs: Option<f64>,
    skip_rate: Option<f64>,
    completion_avg: Option<f64>,
    cohort_id: Option<String>,
    cohort_label: Option<String>,
    top_genre: Option<String>,
    top_genre_source: Option<String>,
    top_genre_confidence: Option<f64>,
    last_played_at: Option<String>,
    play_count: i64,
}
```

- [ ] **Step 2: Initialize new fields to `None` / `0` in the prompt path**

Find the prompt-path mapping starting at `Ok(preview.results.into_iter().map(|r| SpaceTrack {` (around line 1916). Add the new fields after `camelot_key: None,`:

```rust
            Ok(preview.results.into_iter().map(|r| SpaceTrack {
                track_id: r.track_id,
                title: r.title,
                artist_name: r.artist_name.as_deref().unwrap_or("").to_string(),
                album_title: r.album_title,
                artwork_url: r.artwork_url,
                duration_ms: r.duration_ms,
                similarity_score: (r.score as f64 / 99.0).clamp(0.0, 1.0),
                source: r.service,
                energy: None,
                danceability: None,
                bpm: None,
                key_signature: None,
                camelot_key: None,
                is_instrumental: None,
                loudness_lufs: None,
                skip_rate: None,
                completion_avg: None,
                cohort_id: None,
                cohort_label: None,
                top_genre: None,
                top_genre_source: None,
                top_genre_confidence: None,
                last_played_at: None,
                play_count: 0,
            }).collect::<Vec<_>>())
```

- [ ] **Step 3: Same for the radio (seed) path**

Find the radio-path mapping starting at `.map(|t| SpaceTrack {` (around line 1939). Replace the closure body with:

```rust
            .map(|t| SpaceTrack {
                track_id: t.track_id,
                title: t.title,
                artist_name: t.artist_name.unwrap_or_default(),
                album_title: t.album_title,
                artwork_url: t.artwork_url,
                duration_ms: t.duration_ms,
                similarity_score: t.similarity_score,
                source: "tidal".to_string(),
                energy: None,
                danceability: None,
                bpm: None,
                key_signature: None,
                camelot_key: None,
                is_instrumental: None,
                loudness_lufs: None,
                skip_rate: None,
                completion_avg: None,
                cohort_id: None,
                cohort_label: None,
                top_genre: None,
                top_genre_source: None,
                top_genre_confidence: None,
                last_played_at: None,
                play_count: 0,
            })
            .collect()
```

- [ ] **Step 4: Same for the fallback path**

Find the fallback `space_tracks.push(SpaceTrack {` block (around line 1990). Replace with:

```rust
                space_tracks.push(SpaceTrack {
                    track_id: id,
                    title,
                    artist_name: artist.unwrap_or_default(),
                    album_title: album,
                    artwork_url: artwork,
                    duration_ms: dur,
                    similarity_score: 0.5,
                    source: src.unwrap_or_else(|| "tidal".to_string()),
                    energy: None,
                    danceability: None,
                    bpm: None,
                    key_signature: None,
                    camelot_key: None,
                    is_instrumental: None,
                    loudness_lufs: None,
                    skip_rate: None,
                    completion_avg: None,
                    cohort_id: None,
                    cohort_label: None,
                    top_genre: None,
                    top_genre_source: None,
                    top_genre_confidence: None,
                    last_played_at: None,
                    play_count: 0,
                });
```

- [ ] **Step 5: Extend the DSP join SELECT**

Find the DSP-fetch block at `// ── 3. Fetch DSP features for all collected track IDs ──` (around line 2010). Replace the whole block with:

```rust
    // ── 3. Fetch DSP features for all collected track IDs ────────────────────
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        type DspRow = (
            Option<f64>, // energy
            Option<f64>, // danceability
            Option<f64>, // bpm
            Option<String>, // key_signature
            Option<String>, // camelot_key
            Option<i64>, // is_instrumental (0/1)
            Option<f64>, // loudness_lufs
        );
        let dsp_map: std::collections::HashMap<i64, DspRow> = state.db.with_conn(|conn| {
            let sql = format!(
                "SELECT track_id, energy, danceability, bpm, key_signature, camelot_key,
                        is_instrumental, loudness_lufs
                 FROM audio_dsp_features WHERE track_id IN ({ids_csv})"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<f64>>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                    row.get::<_, Option<f64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<f64>>(7)?,
                ))
            })?;
            let mut map = std::collections::HashMap::new();
            for r in rows {
                let (id, energy, dance, bpm, key, camelot, instr, lufs) = r?;
                map.insert(id, (energy, dance, bpm, key, camelot, instr, lufs));
            }
            Ok(map)
        }).unwrap_or_default();

        for t in &mut space_tracks {
            if let Some((energy, dance, bpm, key, camelot, instr, lufs)) = dsp_map.get(&t.track_id) {
                t.energy = *energy;
                t.danceability = *dance;
                t.bpm = *bpm;
                t.key_signature = key.clone();
                t.camelot_key = camelot.clone();
                t.is_instrumental = instr.map(|v| v != 0);
                t.loudness_lufs = *lufs;
            }
        }
    }
```

- [ ] **Step 6: Add new fields to the node JSON output**

Find the node JSON build at the end of the handler (around line 2156, inside `.map(|(i, t)|` returning `json!({ ... })`). Replace the `json!` block with:

```rust
            json!({
                "track_id": t.track_id,
                "title": t.title,
                "artist_name": t.artist_name,
                "album_title": t.album_title,
                "artwork_url": t.artwork_url,
                "duration_ms": t.duration_ms,
                "similarity_score": t.similarity_score,
                "energy": t.energy,
                "danceability": t.danceability,
                "bpm": t.bpm,
                "key_signature": t.key_signature,
                "camelot_key": t.camelot_key,
                "is_instrumental": t.is_instrumental,
                "loudness_lufs": t.loudness_lufs,
                "skip_rate": t.skip_rate,
                "completion_avg": t.completion_avg,
                "cohort_id": t.cohort_id,
                "cohort_label": t.cohort_label,
                "top_genre": t.top_genre,
                "top_genre_source": t.top_genre_source,
                "top_genre_confidence": t.top_genre_confidence,
                "last_played_at": t.last_played_at,
                "play_count": t.play_count,
                "is_in_library": true,
                "source": t.source,
                "x": x,
                "y": y,
                "vx": 0.0,
                "vy": 0.0,
                "radius": node_radius,
                "opacity": 0.0,
            })
```

- [ ] **Step 7: Build & type-check**

Run: `cd E:/NOORwave/noor-server && cargo check 2>&1 | tail -20`
Expected: 0 errors. Warnings about unused fields (`skip_rate`, `cohort_id`, etc.) are OK — they're populated by later tasks.

- [ ] **Step 8: Commit**

```bash
git -C E:/NOORwave add noor-server/src/server/routes.rs
git -C E:/NOORwave commit -m "feat(discovery): extend SpaceTrack with DSP + cohort + skip fields (unwired)"
```

---

### Task 2: Populate skip rate and completion average

**Files:**
- Modify: `noor-server/src/server/routes.rs` (after the DSP block, before the edges block)

- [ ] **Step 1: Add skip-rate aggregation block**

After the closing `}` of the DSP-fetch block from Task 1 (the `for t in &mut space_tracks { ... }` loop), insert this new block:

```rust
    // ── 3b. Aggregate skip-rate + completion-avg from listen_history ─────────
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let listen_map: std::collections::HashMap<i64, (f64, f64)> = state.db.with_conn(|conn| {
            let sql = format!(
                "SELECT lh.track_id,
                        AVG(CASE WHEN lh.completed = 1 THEN 0.0 ELSE 1.0 END) AS skip_rate,
                        AVG(
                            CASE
                                WHEN t.duration_ms IS NULL OR t.duration_ms = 0 THEN NULL
                                ELSE MIN(1.0, CAST(lh.duration_listened_ms AS REAL) / CAST(t.duration_ms AS REAL))
                            END
                        ) AS completion_avg
                 FROM listen_history lh
                 JOIN tracks t ON t.id = lh.track_id
                 WHERE lh.track_id IN ({ids_csv})
                 GROUP BY lh.track_id"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                ))
            })?;
            let mut map = std::collections::HashMap::new();
            for r in rows {
                let (id, skip, comp) = r?;
                map.insert(id, (skip, comp));
            }
            Ok(map)
        }).unwrap_or_default();

        for t in &mut space_tracks {
            if let Some((skip, comp)) = listen_map.get(&t.track_id) {
                t.skip_rate = Some(*skip);
                t.completion_avg = Some(*comp);
            }
        }
    }
```

- [ ] **Step 2: Build & type-check**

Run: `cd E:/NOORwave/noor-server && cargo check 2>&1 | tail -10`
Expected: 0 errors.

- [ ] **Step 3: Commit**

```bash
git -C E:/NOORwave add noor-server/src/server/routes.rs
git -C E:/NOORwave commit -m "feat(discovery): aggregate skip_rate + completion_avg per track"
```

---

### Task 3: Populate `last_played_at`, `play_count`, top genre with source

**Files:**
- Modify: `noor-server/src/server/routes.rs` (after the listen-history block from Task 2)

- [ ] **Step 1: Add the metadata enrichment block**

After the `for t in &mut space_tracks { ... }` loop from Task 2, insert:

```rust
    // ── 3c. Backfill last_played_at + play_count from tracks table ───────────
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let track_meta: std::collections::HashMap<i64, (Option<String>, i64)> = state.db.with_conn(|conn| {
            let sql = format!(
                "SELECT id, last_played_at, play_count FROM tracks WHERE id IN ({ids_csv})"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                ))
            })?;
            let mut map = std::collections::HashMap::new();
            for r in rows {
                let (id, last, plays) = r?;
                map.insert(id, (last, plays));
            }
            Ok(map)
        }).unwrap_or_default();

        for t in &mut space_tracks {
            if let Some((last, plays)) = track_meta.get(&t.track_id) {
                t.last_played_at = last.clone();
                t.play_count = *plays;
            }
        }
    }

    // ── 3d. Top-genre with source + confidence (highest confidence per track) ─
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        type GenreRow = (String, Option<String>, Option<f64>);
        let genre_map: std::collections::HashMap<i64, GenreRow> = state.db.with_conn(|conn| {
            let sql = format!(
                "SELECT tg.track_id, g.name, tg.source, tg.confidence
                 FROM track_genres tg
                 JOIN genres g ON g.id = tg.genre_id
                 WHERE tg.track_id IN ({ids_csv})
                 ORDER BY tg.track_id, COALESCE(tg.confidence, 0) DESC"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<f64>>(3)?,
                ))
            })?;
            let mut map = std::collections::HashMap::new();
            for r in rows {
                let (id, name, source, conf) = r?;
                map.entry(id).or_insert((name, source, conf));
            }
            Ok(map)
        }).unwrap_or_default();

        for t in &mut space_tracks {
            if let Some((name, source, conf)) = genre_map.get(&t.track_id) {
                t.top_genre = Some(name.clone());
                t.top_genre_source = source.clone();
                t.top_genre_confidence = *conf;
            }
        }
    }
```

- [ ] **Step 2: Build & type-check**

Run: `cd E:/NOORwave/noor-server && cargo check 2>&1 | tail -10`
Expected: 0 errors.

- [ ] **Step 3: Commit**

```bash
git -C E:/NOORwave add noor-server/src/server/routes.rs
git -C E:/NOORwave commit -m "feat(discovery): enrich nodes with last_played, play_count, top_genre source"
```

---

### Task 4: Cohort assignment helper + node enrichment

**Files:**
- Create / modify: `noor-server/src/db/queries.rs` — add `get_track_cohort_assignments` helper
- Modify: `noor-server/src/server/routes.rs` — call it from the handler

- [ ] **Step 1: Add the cohort-assignment helper in queries.rs**

In `noor-server/src/db/queries.rs`, locate the end of the `get_genre_cohorts` function (after the closing `}` of that function around line 1500). Insert this helper directly after it:

```rust
/// Map track IDs to their dominant cohort (id, label) using `get_genre_cohorts`.
/// A track is assigned to the cohort whose `genre_ids` contain at least one of
/// its tags; if multiple cohorts match, the one with the highest `listen_count`
/// (already sorted by `get_genre_cohorts`) wins.
pub fn get_track_cohort_assignments(
    conn: &Connection,
    track_ids: &[i64],
    days: i64,
) -> Result<std::collections::HashMap<i64, (String, String)>> {
    if track_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let cohorts = get_genre_cohorts(conn, days)?;
    if cohorts.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // Build genre_id → (cohort_id, cohort_label), preferring earlier (higher-rank) cohorts.
    let mut genre_to_cohort: std::collections::HashMap<i64, (String, String)> =
        std::collections::HashMap::new();
    for cohort in &cohorts {
        for gid in &cohort.genre_ids {
            genre_to_cohort
                .entry(*gid)
                .or_insert((cohort.id.clone(), cohort.label.clone()));
        }
    }

    // Pull all (track_id, genre_id) pairs for the requested tracks.
    let ids_csv: String = track_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT track_id, genre_id FROM track_genres WHERE track_id IN ({ids_csv})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut assignments: std::collections::HashMap<i64, (String, String)> =
        std::collections::HashMap::new();
    for r in rows {
        let (track_id, genre_id) = r?;
        if assignments.contains_key(&track_id) {
            continue;
        }
        if let Some(pair) = genre_to_cohort.get(&genre_id) {
            assignments.insert(track_id, pair.clone());
        }
    }

    Ok(assignments)
}
```

- [ ] **Step 2: Call the helper from the route handler**

In `noor-server/src/server/routes.rs`, after the genre-enrichment block from Task 3 (block `// ── 3d.`), insert:

```rust
    // ── 3e. Cohort assignment per track (90-day window) ──────────────────────
    if !space_tracks.is_empty() {
        let track_ids: Vec<i64> = space_tracks.iter().map(|t| t.track_id).collect();
        let cohort_map: std::collections::HashMap<i64, (String, String)> = state.db.with_conn(|conn| {
            queries::get_track_cohort_assignments(conn, &track_ids, 90)
        }).unwrap_or_default();

        for t in &mut space_tracks {
            if let Some((id, label)) = cohort_map.get(&t.track_id) {
                t.cohort_id = Some(id.clone());
                t.cohort_label = Some(label.clone());
            }
        }
    }
```

- [ ] **Step 3: Build & type-check**

Run: `cd E:/NOORwave/noor-server && cargo check 2>&1 | tail -10`
Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git -C E:/NOORwave add noor-server/src/db/queries.rs noor-server/src/server/routes.rs
git -C E:/NOORwave commit -m "feat(discovery): assign cohort label to space nodes via genre lookup"
```

---

### Task 5: Surface `reason_tags` and score components on edges

**Files:**
- Modify: `noor-server/src/server/routes.rs:2055-2119` (edges block)

- [ ] **Step 1: Replace the edges block**

Find the edges block starting at the comment `// ── 4. Build edges from pre-computed neighbor graph ──` (around line 2055). Replace the entire block from that comment through `};` (the closing brace of the `let edges: Vec<Value> = if track_id_set.len() > 1 { ... } else { vec![] };`) with:

```rust
    // ── 4. Build edges from pre-computed neighbor graph ──────────────────────
    // Emit reason_tags + score components alongside the inferred edge type so
    // the frontend can show why two tracks are connected.
    let track_id_set: HashSet<i64> = space_tracks.iter().map(|t| t.track_id).collect();
    let edges: Vec<Value> = if track_id_set.len() > 1 {
        let ids_csv: String = track_id_set.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
        state.db.with_conn(|conn| {
            let sql = format!(
                "SELECT n.track_id, n.neighbor_track_id, n.score,
                        n.behavioral_score, n.audio_score, n.metadata_score, n.reason_json
                 FROM track_neighbors n
                 WHERE n.track_id IN ({ids_csv}) AND n.neighbor_track_id IN ({ids_csv})
                 ORDER BY n.score DESC
                 LIMIT 300"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, f64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })?;
            let mut result = Vec::new();
            for r in rows { result.push(r?); }
            Ok(result)
        })
        .unwrap_or_default()
        .into_iter()
        .map(|(from_id, to_id, score, behavioral, audio, metadata, reason_json)| {
            // Parse reason_json into a tag list (each entry has at least a "key" or "label" string).
            let parsed: Vec<Value> = reason_json
                .as_deref()
                .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
                .unwrap_or_default();
            let tags: Vec<String> = parsed
                .iter()
                .filter_map(|v| {
                    v.get("key")
                        .and_then(|k| k.as_str())
                        .or_else(|| v.get("label").and_then(|l| l.as_str()))
                        .map(|s| s.to_string())
                })
                .collect();

            // Existing edge-type inference (kept for backward compatibility).
            let edge_type = if tags.iter().any(|t| t == "genre_branch") && audio > 0.4 {
                "harmonic"
            } else if behavioral > 0.4 {
                "behavioural"
            } else if tags.iter().any(|t| t == "artist_affinity") {
                "genre"
            } else if metadata > 0.3 {
                "bpm_match"
            } else {
                "behavioural"
            };

            json!({
                "from_id": from_id,
                "to_id": to_id,
                "type": edge_type,
                "weight": score.clamp(0.0, 1.0),
                "reason_tags": tags,
                "behavioral_score": behavioral,
                "audio_score": audio,
                "metadata_score": metadata,
            })
        })
        .collect()
    } else {
        vec![]
    };
```

- [ ] **Step 2: Build & type-check**

Run: `cd E:/NOORwave/noor-server && cargo check 2>&1 | tail -10`
Expected: 0 errors.

- [ ] **Step 3: Commit**

```bash
git -C E:/NOORwave add noor-server/src/server/routes.rs
git -C E:/NOORwave commit -m "feat(discovery): surface reason_tags + score components on edges"
```

---

### Task 6: Add `/api/discovery/space/meta` endpoint

**Files:**
- Modify: `noor-server/src/server/routes.rs:279` (route registration)
- Modify: `noor-server/src/server/routes.rs` (new handler — placed adjacent to `get_discovery_space`)

- [ ] **Step 1: Register the new route**

Find the line at `routes.rs:279`:

```rust
        .route("/api/discovery/space", post(get_discovery_space))
```

Add a new route immediately after it:

```rust
        .route("/api/discovery/space", post(get_discovery_space))
        .route("/api/discovery/space/meta", get(get_discovery_space_meta))
```

- [ ] **Step 2: Add the handler function**

Immediately after the closing `}` of `get_discovery_space` (around line 2186), insert:

```rust
async fn get_discovery_space_meta(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;

    let total_tracks: i64 = state.db.with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get::<_, i64>(0))
            .map_err(Into::into)
    }).unwrap_or(0);

    let model_row: Option<(String, String, Option<String>, i64)> = state.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT model_key, status, trained_at, dimension
             FROM embedding_models
             WHERE is_active = 1
             ORDER BY trained_at DESC NULLS LAST
             LIMIT 1"
        )?;
        let mut rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }).ok().flatten();

    let (model_key, model_status, trained_at, vector_dim, embedding_count) = match &model_row {
        Some((key, status, trained, dim)) => {
            let count: i64 = state.db.with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM track_embeddings te
                     JOIN embedding_models em ON em.id = te.model_id
                     WHERE em.model_key = ?1",
                    rusqlite::params![key],
                    |row| row.get::<_, i64>(0),
                ).map_err(Into::into)
            }).unwrap_or(0);
            (Some(key.clone()), Some(status.clone()), trained.clone(), Some(*dim), count)
        }
        None => (None, None, None, None, 0),
    };

    let coverage = if total_tracks > 0 {
        embedding_count as f64 / total_tracks as f64
    } else {
        0.0
    };

    Ok(Json(json!({
        "model_key": model_key,
        "model_status": model_status,
        "trained_at": trained_at,
        "vector_dim": vector_dim,
        "neighbor_coverage": coverage,
        "track_count_with_embeddings": embedding_count,
        "track_count_total": total_tracks,
    })))
}
```

- [ ] **Step 3: Verify `get` is imported**

Open `noor-server/src/server/routes.rs` and confirm the axum imports near the top include both `get` and `post`. If `get` is missing from the `axum::routing` import, add it. Look for a line like:

```rust
use axum::routing::{get, post};
```

If only `post` is imported, change it to import both. (Most route files already import both.)

- [ ] **Step 4: Build & type-check**

Run: `cd E:/NOORwave/noor-server && cargo check 2>&1 | tail -15`
Expected: 0 errors.

- [ ] **Step 5: Commit**

```bash
git -C E:/NOORwave add noor-server/src/server/routes.rs
git -C E:/NOORwave commit -m "feat(discovery): add /api/discovery/space/meta endpoint"
```

---

### Task 7: Frontend type extensions for nodes and edges

**Files:**
- Modify: `frontend/src/lib/components/Discover/discover.types.ts`

- [ ] **Step 1: Extend `DiscoverTrackNode` and `DiscoverEdge`**

Replace the contents of `discover.types.ts` with:

```typescript
export type DiscoverViewMode = 'radio' | 'explore' | 'harmonic' | 'energy_arc' | 'samples';

export interface DiscoverTrackNode {
  track_id: number;
  title: string;
  artist_name: string;
  album_title: string | null;
  artwork_url: string | null;
  duration_ms: number | null;
  similarity_score: number;       // 0-1
  energy: number | null;          // 0-1
  danceability: number | null;    // 0-1
  bpm: number | null;
  key_signature: string | null;
  camelot_key: string | null;
  is_in_library: boolean;
  source: 'tidal' | 'external';
  x: number;
  y: number;
  vx: number;
  vy: number;
  radius: number;
  opacity: number;
  // New in Phase 1 — optional, render later in Phase 2
  is_instrumental?: boolean | null;
  loudness_lufs?: number | null;
  skip_rate?: number | null;          // 0-1, 1 = always skipped
  completion_avg?: number | null;     // 0-1, average fraction listened
  cohort_id?: string | null;          // e.g. "night_owl"
  cohort_label?: string | null;       // e.g. "Night Owl"
  top_genre?: string | null;
  top_genre_source?: string | null;   // 'tidal' | 'spotify' | 'musicbrainz' | 'lastfm'
  top_genre_confidence?: number | null;
  last_played_at?: string | null;     // ISO date string
  play_count?: number;
}

export interface DiscoverArtistNode {
  artist_id: number;
  name: string;
  top_genre: string | null;
  affinity: number;               // 0-1
  x: number;
  y: number;
  vx: number;
  vy: number;
  size: number;
}

export interface DiscoverEdge {
  from_id: number;
  to_id: number;
  type: 'bpm_match' | 'harmonic' | 'behavioural' | 'sample' | 'genre';
  weight: number;                 // 0-1
  // New in Phase 1
  reason_tags?: string[];
  behavioral_score?: number;
  audio_score?: number;
  metadata_score?: number;
}

export interface DiscoverySpaceResponse {
  tracks: DiscoverTrackNode[];
  artists: DiscoverArtistNode[];
  edges: DiscoverEdge[];
}

// Phase 1 — meta endpoint
export interface DiscoverySpaceMeta {
  model_key: string | null;
  model_status: string | null;     // 'idle' | 'training' | 'ready' | 'active'
  trained_at: string | null;       // ISO date string
  vector_dim: number | null;
  neighbor_coverage: number;       // 0-1
  track_count_with_embeddings: number;
  track_count_total: number;
}
```

- [ ] **Step 2: Type-check**

Run: `cd E:/NOORwave/frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -8`
Expected: 0 new errors. Pre-existing errors in `src/routes/settings/+page.svelte` about `spotifyTotal` are acceptable.

- [ ] **Step 3: Commit**

```bash
git -C E:/NOORwave add frontend/src/lib/components/Discover/discover.types.ts
git -C E:/NOORwave commit -m "feat(discover): extend node + edge types with Phase 1 fields"
```

---

### Task 8: Frontend API method for `/space/meta` + dev-gated console.log

**Files:**
- Modify: `frontend/src/lib/api/client.ts` — add `getDiscoverySpaceMeta()`
- Modify: `frontend/src/lib/stores/discover_space.ts` — log payload in dev

- [ ] **Step 1: Locate the API client and add the new method**

Open `frontend/src/lib/api/client.ts`. Find an existing discovery-related method (search for `discovery` or `getDiscover`). Adjacent to it (or at the end of the API surface), add:

```typescript
export interface DiscoverySpaceMetaResponse {
  model_key: string | null;
  model_status: string | null;
  trained_at: string | null;
  vector_dim: number | null;
  neighbor_coverage: number;
  track_count_with_embeddings: number;
  track_count_total: number;
}

export async function getDiscoverySpaceMeta(): Promise<DiscoverySpaceMetaResponse> {
  const apiBase = getApiBase();
  const response = await authFetch(`${apiBase}/api/discovery/space/meta`);
  if (!response.ok) {
    throw new Error(`Failed to fetch discovery space meta: ${response.status}`);
  }
  return response.json();
}
```

If `getApiBase` and `authFetch` are not already exported from the same file, place this method wherever its sibling discovery methods live (matching the existing pattern).

- [ ] **Step 2: Add dev-gated payload logging in the store**

Open `frontend/src/lib/stores/discover_space.ts`. Inside `loadSpace`, after the line `const data = await response.json();`, insert a dev-gated console.log so we can validate the wire shape during testing. Replace:

```typescript
            const data = await response.json();
            discoverSpace.update(s => ({
                ...s,
                nodes: data.tracks ?? [],
                edges: data.edges ?? [],
                loading: false,
            }));
```

With:

```typescript
            const data = await response.json();
            if (import.meta.env.DEV) {
                console.log('[discover/space] payload', {
                    sample_node: data.tracks?.[0],
                    sample_edge: data.edges?.[0],
                    node_count: data.tracks?.length ?? 0,
                    edge_count: data.edges?.length ?? 0,
                });
            }
            discoverSpace.update(s => ({
                ...s,
                nodes: data.tracks ?? [],
                edges: data.edges ?? [],
                loading: false,
            }));
```

- [ ] **Step 3: Type-check**

Run: `cd E:/NOORwave/frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -8`
Expected: 0 new errors.

- [ ] **Step 4: Commit**

```bash
git -C E:/NOORwave add frontend/src/lib/api/client.ts frontend/src/lib/stores/discover_space.ts
git -C E:/NOORwave commit -m "feat(discover): add space/meta API method + dev payload log"
```

---

### Task 9: End-to-end smoke verification

**Files:** none modified — this is the integration check.

- [ ] **Step 1: Start the backend**

Run in a terminal: `cd E:/NOORwave/noor-server && cargo run` (let it bind to its usual port; check the logged URL).

- [ ] **Step 2: Hit `/api/discovery/space` with a seed**

Pick any track ID from the library (e.g. open the discover page in a browser to find one, or query the DB). Run:

```bash
curl -s -X POST http://localhost:3010/api/discovery/space \
  -H "Content-Type: application/json" \
  -d '{"mode":"radio","seed_track_id":<TRACK_ID>,"limit":20}' | jq '.tracks[0]'
```

Expected: a node JSON object that includes the new fields. They may be `null` (not all tracks will have DSP / cohort / listen data), but the **keys must be present**:
- `is_instrumental`, `loudness_lufs`
- `skip_rate`, `completion_avg`
- `cohort_id`, `cohort_label`
- `top_genre`, `top_genre_source`, `top_genre_confidence`
- `last_played_at`, `play_count`

- [ ] **Step 3: Inspect an edge**

```bash
curl -s -X POST http://localhost:3010/api/discovery/space \
  -H "Content-Type: application/json" \
  -d '{"mode":"radio","seed_track_id":<TRACK_ID>,"limit":20}' | jq '.edges[0]'
```

Expected (when neighbor data exists): `reason_tags` is a non-empty array, and `behavioral_score`, `audio_score`, `metadata_score` are numeric.

- [ ] **Step 4: Hit the meta endpoint**

```bash
curl -s http://localhost:3010/api/discovery/space/meta | jq
```

Expected: an object with keys `model_key`, `model_status`, `trained_at`, `vector_dim`, `neighbor_coverage`, `track_count_with_embeddings`, `track_count_total`. If no model has been trained yet, `model_key` / `model_status` / `trained_at` may be `null` and `neighbor_coverage` will be `0` — that's correct.

- [ ] **Step 5: Frontend dev-console check**

Run `cd E:/NOORwave/frontend && npx vite dev` in another terminal. Open the browser, navigate to `/discover`, and watch the dev console. You should see one log line per space load:

```
[discover/space] payload { sample_node: {...}, sample_edge: {...}, node_count: 60, edge_count: 84 }
```

Inspect the `sample_node` and `sample_edge` to confirm the new fields are populated.

- [ ] **Step 6: Regression check**

The discover canvas should look identical to before — same nodes, same edges, same animations, same colors. If anything visual has changed, something went wrong.

- [ ] **Step 7: Final commit (optional — for a clean cap on Phase 1)**

If you find any small fixes during smoke verification, commit them with a clear message. Otherwise, no commit is needed for Task 9.
