# Discovery Phase 2 — Seed-Based External Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewire `/api/discovery/space`'s seed path so that giving it a `seed_track_id` returns NEW external Tidal tracks similar to that seed (filtered to non-library), and surface a hybrid auto-seed/lock UI on the discover page.

**Architecture:** The backend already has `/api/discovery/new` doing prompt-based external search via `external_discovery_engine` + `TidalDiscoveryProvider` + `existing_candidate_tidal_ids` library filter. Phase 2 swaps the seed branch in `get_discovery_space` to use `build_connection_queries` (which already accepts a `DiscoveryCandidateSeed`) instead of `radio_from_neighbors`, populates `DiscoveryCandidateSeed` from a library track, and emits external candidates as `SpaceTrack` rows with `is_in_library: false` and `track_id` set to the candidate's Tidal ID. The seed itself is prepended as the first node (library track, `is_in_library: true`) so the canvas has a center. Edges become radial spokes from the seed to each candidate, with weight = candidate's external score.

**Tech Stack:** Rust (axum, rusqlite, async Tidal HTTP), Svelte 5 + TypeScript. Type-check backend with `cd noor-server && cargo check`. Type-check frontend with `cd frontend && npx svelte-check --tsconfig ./tsconfig.json`.

> **Repo convention:** Do NOT add any `Co-Authored-By` trailer to commits. The repo memory says "omit Claude/Anthropic co-author trailers from all commits."

---

## File responsibilities

| File | Role | Touched in tasks |
|---|---|---|
| `noor-server/src/db/queries.rs` | New helper to build a `DiscoveryCandidateSeed` from a library track | T1 |
| `noor-server/src/server/routes.rs` | `get_discovery_space` handler — major rewire of the seed branch + radial edges + Phase 1 enrichment guard | T2, T3, T4, T5, T6 |
| `frontend/src/lib/stores/discover_space.ts` | Add `lockedSeedId`, `activeSeedId`, `lockSeed/unlockSeed` | T7 |
| `frontend/src/routes/discover/+page.svelte` | Hybrid auto-seed effect + lock pill UI + empty state | T8 |

---

### Task 1: Add `load_external_seed_from_track` helper

**Files:**
- Modify: `noor-server/src/db/queries.rs` (add new function)

- [ ] **Step 1: Add the helper function**

Open `noor-server/src/db/queries.rs`. The file already imports the type we need from `crate::services::discovery::DiscoveryCandidateSeed` — verify by running `grep -n "DiscoveryCandidateSeed" noor-server/src/db/queries.rs`. If it's not imported, add this near the existing imports at the top of the file:

```rust
use crate::services::discovery::DiscoveryCandidateSeed;
```

Find a sensible place to add the new function — at the bottom of the file is fine. Add:

```rust
/// Load enough metadata about a library track to seed external Tidal discovery.
/// Returns None if the track id isn't found.
///
/// `provider_track_id` is set from `tracks.tidal_id` if available; otherwise the
/// library `id` is used as a string. `normalized_genres` is the top 5 genres
/// for the track ordered by descending confidence.
pub fn load_external_seed_from_track(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<DiscoveryCandidateSeed>> {
    let row = conn.query_row(
        "SELECT t.id, t.tidal_id, t.title, ar.name, al.title
         FROM tracks t
         LEFT JOIN artists ar ON t.artist_id = ar.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.id = ?1",
        params![track_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        },
    );

    let (id, tidal_id, title, artist_name, album_title) = match row {
        Ok(r) => r,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let mut stmt = conn.prepare(
        "SELECT g.name
         FROM track_genres tg
         JOIN genres g ON g.id = tg.genre_id
         WHERE tg.track_id = ?1
         ORDER BY COALESCE(tg.confidence, 0) DESC
         LIMIT 5",
    )?;
    let genres: Vec<String> = stmt
        .query_map(params![track_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Some(DiscoveryCandidateSeed {
        provider_track_id: tidal_id
            .map(|t| t.to_string())
            .unwrap_or_else(|| id.to_string()),
        title,
        artist_name,
        album_title,
        normalized_genres: genres,
    }))
}
```

- [ ] **Step 2: Build & type-check**

Run: `cd E:/NOORwave/noor-server && cargo check 2>&1 | tail -10`
Expected: 0 errors. Compiler may warn about unused — that's fine since this task ships the helper alone.

- [ ] **Step 3: Commit**

```bash
git -C E:/NOORwave add noor-server/src/db/queries.rs
git -C E:/NOORwave commit -m "feat(discovery): load_external_seed_from_track helper"
```

---

### Task 2: Add `is_in_library` field to `SpaceTrack` + default initializers

**Files:**
- Modify: `noor-server/src/server/routes.rs` (the local `SpaceTrack` struct inside `get_discovery_space` and all 3 of its construction sites)

- [ ] **Step 1: Add `is_in_library: bool` to the SpaceTrack struct**

In `noor-server/src/server/routes.rs`, find the `struct SpaceTrack {` declaration inside `async fn get_discovery_space` (around line 1873). Currently the last fields are:

```rust
        last_played_at: Option<String>,
        play_count: i64,
    }
```

Add a new field right after `play_count`:

```rust
        last_played_at: Option<String>,
        play_count: i64,
        is_in_library: bool,
    }
```

- [ ] **Step 2: Default `is_in_library: true` in the prompt path**

Find the prompt-path mapping (the closure starting `Ok(preview.results.into_iter().map(|r| SpaceTrack {`). The current closure ends with:

```rust
                last_played_at: None,
                play_count: 0,
            }).collect::<Vec<_>>())
```

Add `is_in_library: true,` after `play_count: 0,`:

```rust
                last_played_at: None,
                play_count: 0,
                is_in_library: true,
            }).collect::<Vec<_>>())
```

- [ ] **Step 3: Default `is_in_library: true` in the seed/radio path**

Find the seed-path mapping (the closure `.map(|t| SpaceTrack {` inside `else if seed_id > 0 {`). It currently ends:

```rust
                last_played_at: None,
                play_count: 0,
            })
            .collect()
```

Add `is_in_library: true,`:

```rust
                last_played_at: None,
                play_count: 0,
                is_in_library: true,
            })
            .collect()
```

(This branch will be replaced entirely in Task 4 — but for now we keep the existing library-similarity behavior, just with the new field.)

- [ ] **Step 4: Default `is_in_library: true` in the fallback push**

Find the fallback `space_tracks.push(SpaceTrack {` block. It currently ends:

```rust
                    last_played_at: None,
                    play_count: 0,
                });
```

Add the field:

```rust
                    last_played_at: None,
                    play_count: 0,
                    is_in_library: true,
                });
```

- [ ] **Step 5: Replace the hardcoded `"is_in_library": true` in the JSON output**

Find the per-node `json!({ ... })` block (around line 2225, currently emits `"is_in_library": true,`). Change that line from a literal `true` to read the field:

```rust
                "is_in_library": t.is_in_library,
```

- [ ] **Step 6: Build & type-check**

Run: `cd E:/NOORwave/noor-server && cargo check 2>&1 | tail -10`
Expected: 0 errors.

- [ ] **Step 7: Commit**

```bash
git -C E:/NOORwave add noor-server/src/server/routes.rs
git -C E:/NOORwave commit -m "feat(discovery): SpaceTrack.is_in_library field (defaults true)"
```

---

### Task 3: Rename the `state` shadow guard so async helpers stay reachable

**Files:**
- Modify: `noor-server/src/server/routes.rs` (the read-guard binding inside `get_discovery_space`)

**Why:** The seed branch (Task 4) needs to call `tidal_discovery_provider(&state)`, `load_external_discovery_context(&state)`, etc. Those expect `&State<SharedState>`. The handler currently shadows `state` with a read guard via `let state = state.read().await;` (around line 1870), which makes the original `State<SharedState>` parameter unreachable. We rename the guard so both bindings coexist.

- [ ] **Step 1: Rename the read-guard binding**

In `noor-server/src/server/routes.rs`, inside `async fn get_discovery_space`, find the line (around line 1870):

```rust
    let state = state.read().await;
```

Replace with:

```rust
    let state_guard = state.read().await;
```

- [ ] **Step 2: Update every `state.db.with_conn` call inside this function to `state_guard.db.with_conn`**

There are several. Use search-and-replace within the function body of `get_discovery_space` (do NOT touch other handlers in the file). The pattern `state.db.with_conn` becomes `state_guard.db.with_conn`. Also any direct `state.db` references inside this function become `state_guard.db`.

Specifically, look for these patterns within the function body (between the function's opening `{` after the signature and its closing `}` before `#[derive(Debug, Deserialize)] struct DiscoveryArtistsQuery`):

- `state.db.with_conn(` → `state_guard.db.with_conn(`
- `&state.db` → `&state_guard.db`

After this rename, the original parameter `state: State<SharedState>` (introduced by `State(state): State<SharedState>` in the signature) is shadowed only by the new `state_guard` binding for the read guard. The unshadowed `state` remains accessible for async helper calls.

- [ ] **Step 3: Build & type-check**

Run: `cd E:/NOORwave/noor-server && cargo check 2>&1 | tail -15`
Expected: 0 errors. If there are errors about `state` being moved or about types not matching, double-check that all references inside the function were renamed. If a reference outside the function got accidentally changed, revert that one.

- [ ] **Step 4: Commit**

```bash
git -C E:/NOORwave add noor-server/src/server/routes.rs
git -C E:/NOORwave commit -m "refactor(discovery): rename get_discovery_space read guard to state_guard"
```

---

### Task 4: Replace the seed branch with seed-based external discovery

**Files:**
- Modify: `noor-server/src/server/routes.rs` (the `else if seed_id > 0 { ... }` branch inside `get_discovery_space`)

- [ ] **Step 1: Replace the entire seed branch**

In `get_discovery_space`, find the seed branch. Currently (after Task 3's rename it should look something like):

```rust
    } else if seed_id > 0 {
        let creativity = payload.creativity.unwrap_or(0.3).clamp(0.0, 1.0);
        discovery_learning::radio_from_neighbors(&state_guard.db, seed_id, &[], limit, creativity)
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
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
                is_in_library: true,
            })
            .collect()
    } else {
        vec![]
    };
```

Replace the entire `} else if seed_id > 0 {` block (through the inner `.collect()`) with:

```rust
    } else if seed_id > 0 {
        // Load the seed's metadata from the library so we can build Tidal queries.
        let seed_opt = state_guard.db.with_conn(|conn| {
            queries::load_external_seed_from_track(conn, seed_id)
        }).ok().flatten();

        if let Some(seed_meta) = seed_opt {
            // Drop the read guard so async helpers can take their own locks.
            // (We re-acquire later for Phase 1 enrichment passes.)
            drop(state_guard);

            let request = external_discovery_engine::ExternalDiscoveryRequest {
                prompt: String::new(),
                mode: mode.clone(),
                services: vec!["tidal".to_string()],
                limit: limit as usize,
            };

            let context = match load_external_discovery_context(&state).await {
                Ok(c) => c,
                Err(_) => {
                    state_guard = state.read().await;
                    return Ok(Json(json!({ "tracks": [], "artists": [], "edges": [] })));
                }
            };

            let queries = external_discovery_engine::build_connection_queries(
                &request,
                &context,
                &seed_meta,
            );
            let queries = augment_search_queries_with_lastfm(&state, &request, &context, queries).await;

            let provider = match tidal_discovery_provider(&state).await {
                Ok(p) => p,
                Err(_) => {
                    state_guard = state.read().await;
                    return Ok(Json(json!({ "tracks": [], "artists": [], "edges": [] })));
                }
            };

            let raw = provider.search_tracks(&queries, 8).await.unwrap_or_default();
            let candidates = enrich_candidates_with_metadata(&state, raw).await;
            let library_tidal_ids = existing_candidate_tidal_ids(&state, &candidates)
                .await
                .unwrap_or_default();

            let feed = external_discovery_engine::build_external_feed(
                &request,
                &context,
                &candidates,
                &library_tidal_ids,
                discovery_provider_capabilities(),
                None,
            );

            // Re-acquire the read guard for the enrichment passes that follow.
            state_guard = state.read().await;

            feed.results
                .into_iter()
                .filter_map(|r| {
                    let tidal_id = r.tidal_track_id?;
                    Some(SpaceTrack {
                        track_id: tidal_id,
                        title: r.title,
                        artist_name: r.artist_name.unwrap_or_default(),
                        album_title: r.album_title,
                        artwork_url: r.artwork_url,
                        duration_ms: r.duration_ms,
                        similarity_score: (r.score as f64 / 99.0).clamp(0.0, 1.0),
                        source: "external".to_string(),
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
                        is_in_library: false,
                    })
                })
                .collect()
        } else {
            // Seed not found — empty result.
            vec![]
        }
    } else {
        vec![]
    };
```

The `state_guard` binding needs to be `let mut state_guard = ...` for the reassignment to work. In Task 3 you set `let state_guard = state.read().await;` — change that to `let mut state_guard = state.read().await;`.

- [ ] **Step 2: Make `state_guard` mutable**

Find the line `let state_guard = state.read().await;` (added in Task 3) and change to:

```rust
    let mut state_guard = state.read().await;
```

- [ ] **Step 3: Build & type-check**

Run: `cd E:/NOORwave/noor-server && cargo check 2>&1 | tail -20`
Expected: 0 errors. Common issues:
- If `mode.clone()` complains, the existing `let mode = ...` is `String`, so `.clone()` works.
- If `external_discovery_engine` isn't already imported in this file's top-level imports, find an existing reference inside the handler (e.g. `discovery_engine::DiscoveryPreviewRequest` line 1900) and add `external_discovery_engine` to the same `use crate::smart::{ ... };` line, or add `use crate::smart::external_discovery as external_discovery_engine;` near the top of the file.

- [ ] **Step 4: Commit**

```bash
git -C E:/NOORwave add noor-server/src/server/routes.rs
git -C E:/NOORwave commit -m "feat(discovery): seed_track_id now drives external Tidal search"
```

---

### Task 5: Prepend the seed track + build radial edges

**Files:**
- Modify: `noor-server/src/server/routes.rs` (the seed branch + the edges block)

- [ ] **Step 1: Prepend the seed track to space_tracks**

Right after the seed branch closes (the `let mut space_tracks: Vec<SpaceTrack> = if !prompt.is_empty() { ... } else if seed_id > 0 { ... } else { vec![] };` statement), before the `// ── 2. Fill remainder from most-played ──` comment, insert:

```rust
    // ── 1b. Prepend the seed track itself when in seed mode (so canvas has center) ──
    if seed_id > 0 && prompt.is_empty() {
        // Avoid duplicating if it somehow ended up in the candidate list.
        let already_present = space_tracks.iter().any(|t| t.track_id == seed_id);
        if !already_present {
            let seed_track_opt = state_guard.db.with_conn(|conn| {
                conn.query_row(
                    "SELECT t.id, t.title, ar.name, al.title, al.artwork_url, t.duration_ms, t.source
                     FROM tracks t
                     LEFT JOIN artists ar ON t.artist_id = ar.id
                     LEFT JOIN albums al ON t.album_id = al.id
                     WHERE t.id = ?1",
                    rusqlite::params![seed_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<i64>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                        ))
                    },
                ).ok()
            }).unwrap_or(None);

            if let Some((id, title, artist, album, artwork, dur, src)) = seed_track_opt {
                space_tracks.insert(0, SpaceTrack {
                    track_id: id,
                    title,
                    artist_name: artist.unwrap_or_default(),
                    album_title: album,
                    artwork_url: artwork,
                    duration_ms: dur,
                    similarity_score: 1.0,
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
                    is_in_library: true,
                });
            }
        }
    }
```

- [ ] **Step 2: Override the edges block for seed-based external mode**

Find the edges block (`// ── 4. Build edges from pre-computed neighbor graph ──` around line 2275 after Task 4 changes). Wrap the existing edges-building code so it only runs when we're NOT in seed-based external mode. In seed-based external mode, we build radial spokes from the seed to every external candidate instead.

Replace:

```rust
    // ── 4. Build edges from pre-computed neighbor graph ──────────────────────
    // Emit reason_tags + score components alongside the inferred edge type so
    // the frontend can show why two tracks are connected.
    let track_id_set: HashSet<i64> = space_tracks.iter().map(|t| t.track_id).collect();
    let edges: Vec<Value> = if track_id_set.len() > 1 {
```

With:

```rust
    // ── 4. Build edges ───────────────────────────────────────────────────────
    // Seed-based external mode: radial spokes from seed → each external candidate.
    // Otherwise: pull from the pre-computed neighbor graph (Phase 1 behavior).
    let is_external_seed_mode = seed_id > 0
        && prompt.is_empty()
        && space_tracks.iter().any(|t| !t.is_in_library);

    let edges: Vec<Value> = if is_external_seed_mode {
        space_tracks
            .iter()
            .filter(|t| !t.is_in_library)
            .map(|t| {
                json!({
                    "from_id": seed_id,
                    "to_id": t.track_id,
                    "type": "behavioural",
                    "weight": t.similarity_score,
                    "reason_tags": ["external_match"],
                    "behavioral_score": 0.0,
                    "audio_score": 0.0,
                    "metadata_score": t.similarity_score,
                })
            })
            .collect()
    } else if {
        let track_id_set: HashSet<i64> = space_tracks.iter().map(|t| t.track_id).collect();
        track_id_set.len() > 1
    } {
```

Wait — that creates a syntax issue with the `else if {}` block. Let me restructure. Replace the whole block from the comment `// ── 4. Build edges` through the closing `};` of `let edges: Vec<Value> = ...` with this cleaner version:

```rust
    // ── 4. Build edges ───────────────────────────────────────────────────────
    // Seed-based external mode: radial spokes from seed → each external candidate.
    // Otherwise: pull from the pre-computed neighbor graph (Phase 1 behavior).
    let is_external_seed_mode = seed_id > 0
        && prompt.is_empty()
        && space_tracks.iter().any(|t| !t.is_in_library);

    let edges: Vec<Value> = if is_external_seed_mode {
        space_tracks
            .iter()
            .filter(|t| !t.is_in_library)
            .map(|t| {
                json!({
                    "from_id": seed_id,
                    "to_id": t.track_id,
                    "type": "behavioural",
                    "weight": t.similarity_score,
                    "reason_tags": ["external_match"],
                    "behavioral_score": 0.0,
                    "audio_score": 0.0,
                    "metadata_score": t.similarity_score,
                })
            })
            .collect()
    } else {
        let track_id_set: HashSet<i64> = space_tracks.iter().map(|t| t.track_id).collect();
        if track_id_set.len() > 1 {
            let ids_csv: String = track_id_set.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
            state_guard.db.with_conn(|conn| {
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
        }
    };
```

- [ ] **Step 3: Build & type-check**

Run: `cd E:/NOORwave/noor-server && cargo check 2>&1 | tail -15`
Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git -C E:/NOORwave add noor-server/src/server/routes.rs
git -C E:/NOORwave commit -m "feat(discovery): prepend seed node + radial edges for external mode"
```

---

### Task 6: Skip Phase 1 enrichment for external (non-library) tracks

**Files:**
- Modify: `noor-server/src/server/routes.rs` (the four enrichment blocks: 3, 3b, 3c, 3d, 3e)

**Why:** The Phase 1 enrichment passes (DSP join, listen-history, last-played, top-genre, cohort) all `SELECT ... WHERE track_id IN (...)` against the library DB tables. External nodes use Tidal IDs (large positive integers like 12345678) as their `track_id`. A library track with id 12345678 is unlikely but possible — and even more importantly, running an unnecessary IN-list query against possibly-thousands of external Tidal IDs is wasteful. Restrict the enrichment to library tracks only.

- [ ] **Step 1: Update each enrichment block's `ids_csv` builder to filter to library tracks**

There are five blocks to update:
- `// ── 3. Fetch DSP features for all collected track IDs ──` (DSP)
- `// ── 3b. Aggregate skip-rate + completion-avg from listen_history ──`
- `// ── 3c. Backfill last_played_at + play_count from tracks table ──`
- `// ── 3d. Top-genre with source + confidence ──`
- `// ── 3e. Cohort assignment per track (90-day window) ──`

In each block, find the line that builds `ids_csv`:

```rust
        let ids_csv: String = space_tracks
            .iter()
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");
```

Add an `is_in_library` filter:

```rust
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");
```

Repeat this exact change in all five blocks.

For block 3e (cohort assignment), the change is slightly different because it builds a `Vec<i64>` not a CSV. Find:

```rust
        let track_ids: Vec<i64> = space_tracks.iter().map(|t| t.track_id).collect();
```

Change to:

```rust
        let track_ids: Vec<i64> = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id)
            .collect();
```

- [ ] **Step 2: Guard against empty filtered ID lists**

Each block runs `if !space_tracks.is_empty() { ... }`. After the Task 6 Step 1 change, even when `space_tracks` is non-empty the filtered `ids_csv` (or `track_ids`) can be empty. The SQL `WHERE id IN ()` with an empty `()` is invalid in SQLite.

In each of the five blocks, add an early-return guard right after the `ids_csv` construction (or `track_ids` for block 3e). For the four CSV-based blocks:

```rust
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        if ids_csv.is_empty() {
            // No library tracks present (pure external response) — nothing to enrich.
        } else {
            // ... existing block body that uses ids_csv ...
        }
```

Wrap the existing `state_guard.db.with_conn(|conn| { ... }).unwrap_or_default();` (and the for-loop that follows) in the `else` branch.

For block 3e (cohort), the equivalent guard is:

```rust
        if track_ids.is_empty() {
            // No library tracks — skip cohort assignment.
        } else {
            // ... existing block body that uses track_ids ...
        }
```

- [ ] **Step 3: Build & type-check**

Run: `cd E:/NOORwave/noor-server && cargo check 2>&1 | tail -10`
Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git -C E:/NOORwave add noor-server/src/server/routes.rs
git -C E:/NOORwave commit -m "feat(discovery): restrict Phase 1 enrichment to library tracks only"
```

---

### Task 7: Frontend store — `lockedSeedId` state + lock/unlock helpers

**Files:**
- Modify: `frontend/src/lib/stores/discover_space.ts`

- [ ] **Step 1: Replace the file**

Open `frontend/src/lib/stores/discover_space.ts`. Replace the entire file with:

```typescript
import { writable } from 'svelte/store';
import { getApiBase, authFetch } from '$lib/api/client';
import type { DiscoverTrackNode, DiscoverEdge, DiscoverViewMode } from '$lib/components/Discover/discover.types';

interface DiscoverSpaceState {
	mode: DiscoverViewMode;
	nodes: DiscoverTrackNode[];
	edges: DiscoverEdge[];
	loading: boolean;
	visitedRegions: Map<string, { x: number; y: number; radius: number }>;
	// Phase 2: seed management
	lockedSeedId: number | null;     // user-pinned seed; takes precedence over playing
	activeSeedId: number | null;     // resolved seed actually used in the last load
	activeSeedSource: 'locked' | 'playing' | null;
}

export const discoverSpace = writable<DiscoverSpaceState>({
	mode: 'radio',
	nodes: [],
	edges: [],
	loading: false,
	visitedRegions: new Map(),
	lockedSeedId: null,
	activeSeedId: null,
	activeSeedSource: null,
});

export async function loadSpace(
	mode: DiscoverViewMode,
	seedTrackId?: number,
	prompt?: string,
	seedSource: 'locked' | 'playing' | null = null,
) {
	discoverSpace.update(s => ({
		...s,
		loading: true,
		mode,
		activeSeedId: seedTrackId ?? null,
		activeSeedSource: seedSource,
	}));
	try {
		const apiBase = getApiBase();
		const response = await authFetch(`${apiBase}/api/discovery/space`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({
				mode,
				seed_track_id: seedTrackId,
				prompt,
				limit: 60,
				include_artists: mode === 'explore',
			}),
		});

		if (!response.ok) {
			throw new Error(`Failed to load discovery space: ${response.status}`);
		}

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
	} catch (e) {
		console.error('Failed to load discovery space:', e);
		discoverSpace.update(s => ({ ...s, loading: false }));
	}
}

export function lockSeed(trackId: number) {
	discoverSpace.update(s => ({ ...s, lockedSeedId: trackId }));
}

export function unlockSeed() {
	discoverSpace.update(s => ({ ...s, lockedSeedId: null }));
}

export function addVisitedRegion(prompt: string, centroid: { x: number; y: number; radius: number }) {
	discoverSpace.update(s => {
		s.visitedRegions.set(prompt, centroid);
		return s;
	});
}
```

- [ ] **Step 2: Type-check**

Run: `cd E:/NOORwave/frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -10`
Expected: 0 NEW errors. Pre-existing errors about `spotifyTotal` in settings page are acceptable.

- [ ] **Step 3: Commit**

```bash
git -C E:/NOORwave add frontend/src/lib/stores/discover_space.ts
git -C E:/NOORwave commit -m "feat(discover): lockedSeedId state + lockSeed/unlockSeed helpers"
```

---

### Task 8: Frontend page — hybrid auto-seed effect + lock pill UI + empty state

**Files:**
- Modify: `frontend/src/routes/discover/+page.svelte`

- [ ] **Step 1: Replace the script block**

Open `frontend/src/routes/discover/+page.svelte`. Replace the existing `<script lang="ts">...</script>` block (lines 1–64) with:

```svelte
<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { currentTrack, automixEnabled, automixDiscoverNew, automixUseLearning } from '$lib/stores/player';
	import { discoverSpace, loadSpace, lockSeed, unlockSeed } from '$lib/stores/discover_space';
	import DiscoverSpace from '$lib/components/Discover/DiscoverSpace.svelte';
	import DiscoverFilters from '$lib/components/Discover/DiscoverFilters.svelte';
	import DiscoverPanel from '$lib/components/Discover/DiscoverPanel.svelte';
	import PlaylistBuilder from '$lib/components/Discover/PlaylistBuilder.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import type { DiscoverTrackNode, DiscoverViewMode } from '$lib/components/Discover/discover.types';

	let selectedNodes = $state<DiscoverTrackNode[]>([]);
	let panelNode = $state<DiscoverTrackNode | null>(null);
	let searchQuery = $state('');
	let isSearching = $state(false);

	// Resolved seed = locked seed if any, else playing track id, else null.
	let resolvedSeedId = $derived(
		$discoverSpace.lockedSeedId ?? $currentTrack?.id ?? null
	);
	let resolvedSeedSource = $derived<'locked' | 'playing' | null>(
		$discoverSpace.lockedSeedId !== null
			? 'locked'
			: $currentTrack?.id != null
				? 'playing'
				: null
	);

	// Track what seed was last loaded so we don't refetch on every reactive tick.
	let lastLoadedSeedId = $state<number | null>(null);

	function handleModeChange(mode: DiscoverViewMode) {
		lastLoadedSeedId = resolvedSeedId;
		if (resolvedSeedId !== null) {
			loadSpace(mode, resolvedSeedId, undefined, resolvedSeedSource);
		} else {
			discoverSpace.update(s => ({ ...s, mode }));
		}
	}

	function handleHover(_node: DiscoverTrackNode | null) {}

	function handleSelect(node: DiscoverTrackNode) {
		selectedNodes = [...selectedNodes, node];
		panelNode = node;
	}

	function handleNewNodes(incoming: DiscoverTrackNode[]) {
		discoverSpace.update(s => ({ ...s, nodes: [...s.nodes, ...incoming] }));
	}

	async function handleSearch(e: Event) {
		e.preventDefault();
		const q = searchQuery.trim();
		if (!q) return;
		isSearching = true;
		const fn = (window as any).__discoverSpaceHyperspaceSearch;
		if (fn) await fn(q);
		isSearching = false;
		searchQuery = '';
	}

	function handleToggleLock() {
		if ($discoverSpace.lockedSeedId !== null) {
			unlockSeed();
		} else {
			const trackId = $currentTrack?.id;
			if (trackId != null) lockSeed(trackId);
		}
	}

	// Hybrid auto-seed: refetch when the resolved seed changes.
	$effect(() => {
		const seedId = resolvedSeedId;
		if (seedId !== null && seedId !== lastLoadedSeedId) {
			lastLoadedSeedId = seedId;
			loadSpace($discoverSpace.mode, seedId, undefined, resolvedSeedSource);
		}
	});

	onMount(() => {
		const seedId = resolvedSeedId;
		if (seedId !== null) {
			lastLoadedSeedId = seedId;
			loadSpace('radio', seedId, undefined, resolvedSeedSource);
		}
	});
</script>
```

- [ ] **Step 2: Add the lock pill + empty state to the template**

Find the `<div class="discover-header">` block. Right after its closing `</div>` (and before the `{#if $automixEnabled}` block, around line 86), insert:

```svelte
	{#if $discoverSpace.activeSeedId !== null}
		<div class="seed-pill">
			<span class="seed-source">
				{$discoverSpace.activeSeedSource === 'locked' ? '🔒 Locked seed' : '▶ Auto-seeded from playing'}
			</span>
			{#if $currentTrack && $currentTrack.id === $discoverSpace.activeSeedId}
				<span class="seed-title">{$currentTrack.title}</span>
			{/if}
			<button
				class="seed-toggle"
				onclick={handleToggleLock}
				disabled={$currentTrack?.id == null && $discoverSpace.lockedSeedId === null}
			>
				{$discoverSpace.lockedSeedId !== null ? 'Unlock' : 'Lock seed'}
			</button>
		</div>
	{/if}
```

Then find the `<div class="discover-layout">` block. The discover space and panel are inside it. Find the existing `<DiscoverSpace ... />` line — wrap it in a conditional for the empty state:

The current pattern roughly (around line 100+) looks like:

```svelte
	<div class="discover-layout">
		<div class="discover-sidebar">
			<DiscoverFilters
				...
			/>
		</div>
		<div class="discover-canvas">
			<DiscoverSpace ... />
		</div>
		...
	</div>
```

Wrap the `<DiscoverSpace ... />` element so that when there's no seed at all, an `EmptyState` shows instead. Find the `<DiscoverSpace` line and the `<div class="discover-canvas">` it's inside. Update the canvas div to:

```svelte
		<div class="discover-canvas">
			{#if resolvedSeedId === null}
				<EmptyState
					title="Play something to start discovering"
					copy="Discover finds new music similar to whatever you're playing. Hit play, or lock a track as your seed."
				/>
			{:else}
				<DiscoverSpace
					mode={$discoverSpace.mode}
					onHover={handleHover}
					onSelect={handleSelect}
					onNewNodes={handleNewNodes}
				/>
			{/if}
		</div>
```

(If the existing `<DiscoverSpace ...>` has different props, preserve them — just wrap it in the `{#if/else}`.)

- [ ] **Step 3: Add CSS for the seed pill**

Find the `<style>` block. Add at the end of it (before the closing `</style>`):

```css
	.seed-pill {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 8px 14px;
		margin: 0 0 14px 0;
		border-radius: 999px;
		background: rgba(91, 78, 248, 0.08);
		border: 1px solid rgba(91, 78, 248, 0.3);
		font-size: 0.78rem;
		width: fit-content;
	}
	.seed-source {
		color: var(--text-secondary, #a0a0c0);
		letter-spacing: 0.04em;
	}
	.seed-title {
		font-weight: 600;
		color: var(--text-primary, #e8e8f0);
	}
	.seed-toggle {
		margin-left: auto;
		background: transparent;
		border: 1px solid rgba(91, 78, 248, 0.5);
		color: #a0a0e8;
		border-radius: 999px;
		padding: 4px 12px;
		font-size: 0.72rem;
		cursor: pointer;
		transition: background 0.15s, color 0.15s;
	}
	.seed-toggle:hover:not(:disabled) {
		background: rgba(91, 78, 248, 0.2);
		color: #fff;
	}
	.seed-toggle:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}
```

- [ ] **Step 4: Type-check**

Run: `cd E:/NOORwave/frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | tail -10`
Expected: 0 NEW errors. The pre-existing `spotifyTotal` errors are acceptable.

- [ ] **Step 5: Commit**

```bash
git -C E:/NOORwave add frontend/src/routes/discover/+page.svelte
git -C E:/NOORwave commit -m "feat(discover): hybrid auto-seed + lock pill + empty state"
```

---

### Task 9: End-to-end smoke verification

**Files:** none modified — integration check only.

- [ ] **Step 1: Start the backend**

In a terminal: `cd E:/NOORwave/noor-server && cargo run`. Wait for the bound URL to print (e.g. `http://localhost:3010`).

- [ ] **Step 2: Start the frontend**

In a separate terminal: `cd E:/NOORwave/frontend && npx vite dev`.

- [ ] **Step 3: Smoke the API directly**

Pick a library track ID from your DB (e.g. browse the library page in the running app and copy a track id, or query the DB directly). Then:

```bash
curl -s -X POST http://localhost:3010/api/discovery/space \
  -H "Content-Type: application/json" \
  -d '{"mode":"radio","seed_track_id":<TRACK_ID>,"limit":20}' | jq '{
    seed: .tracks[0],
    first_external: (.tracks | map(select(.is_in_library == false)) | .[0]),
    edge_count: (.edges | length),
    sample_edge: .edges[0]
  }'
```

Expected:
- `seed` is the seed track (`is_in_library: true`, library DB id).
- `first_external` is a Tidal track NOT in your library (`is_in_library: false`, `source: "external"`).
- `edge_count` is roughly the number of external candidates (radial spokes).
- `sample_edge` has `from_id` matching the seed's track_id, `to_id` matching an external candidate's tidal_id, and `reason_tags: ["external_match"]`.

- [ ] **Step 4: Browser smoke**

Open the app, play a library track, then navigate to `/discover`. The canvas should populate with new external Tidal tracks similar to the playing track. The seed pill at the top should read `▶ Auto-seeded from playing` with the track's title.

Click "Lock seed". The pill changes to `🔒 Locked seed`. Now skip to a different playing track — the canvas should NOT re-fetch.

Click "Unlock". The pill goes back to auto. Skip again — canvas updates to the new playing track.

Stop playback completely. The canvas should clear and the empty state ("Play something to start discovering") should appear.

- [ ] **Step 5: Regression check on prompt mode**

Use the search box to type something like "dark ambient" and submit. The hyperspace-search path (`/api/discovery/space` with prompt) should still work as before — text-based discovery is unchanged.

- [ ] **Step 6: No commit needed**

Verification only. If you find any small issues, commit fixes with descriptive messages.
