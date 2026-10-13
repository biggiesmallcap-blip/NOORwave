//! Prototype station design under evaluation in the radio blind test.
//!
//! TIDAL's track radio and Last.fm's similar tracks form the pool, each in its
//! own ranking. Your listening and taste reorder that pool, and library tracks
//! your listening links to the seed fill a capped share the providers missed.
//! The Engine index (album and artist siblings) only feeds Familiar. The radio
//! routes do not use this yet; `/api/radio/compare` shows it as one station.

use super::*;

/// A pick both TIDAL and Last.fm list for the seed.
const PROVIDER_AGREEMENT_BONUS: f64 = 0.25;
/// A pool pick your own listening also links to the seed.
const LEARNED_AGREEMENT_BONUS: f64 = 0.15;
/// A pool pick you have liked.
const LIKED_BONUS: f64 = 0.05;

/// Share of the station for library tracks the providers missed, and for
/// Engine siblings.
fn extra_shares(blend: RadioBlend) -> (f64, f64) {
    match blend {
        RadioBlend::Familiar => (0.35, 0.15),
        RadioBlend::Mixed => (0.25, 0.0),
        RadioBlend::Adventurous => (0.10, 0.0),
    }
}

/// The lists a hybrid station is built from.
#[derive(Debug, Default)]
pub struct HybridPool {
    /// TIDAL's track radio, in TIDAL's order.
    pub tidal: Vec<RadioCandidate>,
    /// Last.fm's similar tracks, in Last.fm's order.
    pub lastfm: Vec<RadioCandidate>,
    /// The seed's learned neighbors that listening supports, best first.
    pub learned: Vec<RadioCandidate>,
    /// Engine siblings, best first (Familiar only).
    pub engine: Vec<RadioCandidate>,
}

/// Score from a position in a provider's own list: its order is its ranking,
/// and two providers' raw scores are not comparable.
fn rank_score(index: usize, len: usize) -> f64 {
    1.0 - 0.5 * index as f64 / len.max(1) as f64
}

fn by_score_desc(a: &RadioCandidate, b: &RadioCandidate) -> std::cmp::Ordering {
    b.similarity_score
        .partial_cmp(&a.similarity_score)
        .unwrap_or(std::cmp::Ordering::Equal)
}

/// Build the station from its pool. Pure: everything it reads is passed in.
pub fn blend_hybrid(
    pool: HybridPool,
    liked: &HashSet<i64>,
    taste: &TasteVector,
    resolver: &ArtistResolver,
    blend: RadioBlend,
    limit: usize,
) -> Vec<RadioCandidate> {
    let mut core: Vec<RadioCandidate> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let tidal_len = pool.tidal.len();
    for (index, mut pick) in pool.tidal.into_iter().enumerate() {
        let key = normalize_for_dedup(&pick.artist_name, &pick.title);
        if key.is_empty() || seen.contains_key(&key) {
            continue;
        }
        pick.similarity_score = rank_score(index, tidal_len);
        seen.insert(key, core.len());
        core.push(pick);
    }
    let lastfm_len = pool.lastfm.len();
    for (index, mut pick) in pool.lastfm.into_iter().enumerate() {
        let key = normalize_for_dedup(&pick.artist_name, &pick.title);
        if key.is_empty() {
            continue;
        }
        let score = rank_score(index, lastfm_len);
        if let Some(&at) = seen.get(&key) {
            // Both providers list it: keep TIDAL's playable row, lift it.
            let kept = &mut core[at];
            if kept.source == RadioSource::Tidal {
                kept.similarity_score = kept.similarity_score.max(score) + PROVIDER_AGREEMENT_BONUS;
            }
            continue;
        }
        pick.similarity_score = score;
        seen.insert(key, core.len());
        core.push(pick);
    }

    let learned_ids: HashSet<i64> = pool
        .learned
        .iter()
        .map(|c| c.track_id)
        .filter(|id| *id > 0)
        .collect();
    for pick in core.iter_mut().filter(|c| c.track_id > 0) {
        if learned_ids.contains(&pick.track_id) {
            pick.similarity_score += LEARNED_AGREEMENT_BONUS;
        }
        if liked.contains(&pick.track_id) {
            pick.similarity_score += LIKED_BONUS;
        }
    }
    // Recent early skips leave; artist affinity nudges up or down.
    apply_taste_signals(&mut core, taste, resolver);
    core.sort_by(by_score_desc);

    let (library_share, engine_share) = extra_shares(blend);
    let library_cap = (limit as f64 * library_share).round() as usize;
    let engine_cap = (limit as f64 * engine_share).round() as usize;
    let mut extras: Vec<RadioCandidate> = Vec::new();
    for (list, cap) in [(pool.learned, library_cap), (pool.engine, engine_cap)] {
        let mut taken = 0;
        for pick in list {
            if taken >= cap {
                break;
            }
            let key = normalize_for_dedup(&pick.artist_name, &pick.title);
            if key.is_empty()
                || seen.contains_key(&key)
                || taste.skipped_track_ids.contains(&pick.track_id)
            {
                continue;
            }
            seen.insert(key, usize::MAX);
            extras.push(pick);
            taken += 1;
        }
    }

    // Spread the extras through the station instead of stacking them at the end.
    let total = (core.len() + extras.len()).min(limit);
    let extra_share = if total == 0 {
        0.0
    } else {
        extras.len().min(total) as f64 / total as f64
    };
    let mut core = core.into_iter();
    let mut extras = extras.into_iter();
    let mut placed_extras = 0usize;
    let mut station = Vec::with_capacity(total);
    while station.len() < total {
        let due = ((station.len() + 1) as f64 * extra_share).round() as usize;
        let next = if placed_extras < due {
            extras
                .next()
                .inspect(|_| placed_extras += 1)
                .or_else(|| core.next())
        } else {
            core.next()
                .or_else(|| extras.next().inspect(|_| placed_extras += 1))
        };
        match next {
            Some(pick) => station.push(pick),
            None => break,
        }
    }
    station
}

/// The hybrid station for a seed, from provider lists the caller fetched.
pub async fn hybrid_station(
    db: &Database,
    seed_track_id: i64,
    tidal: Vec<RadioCandidate>,
    lastfm: Vec<RadioCandidate>,
    blend: RadioBlend,
    limit: usize,
) -> Vec<RadioCandidate> {
    let recall_seed_id = crate::services::learning::recall_seed_for(db, seed_track_id);
    let exclude = [seed_track_id, recall_seed_id];
    let learned: Vec<RadioCandidate> =
        crate::services::learning::radio_from_neighbors(db, recall_seed_id, &exclude, 60, 0.0)
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .filter(|n| n.support_count > 0)
            .map(|n| RadioCandidate {
                track_id: n.track_id,
                tidal_track_id: None,
                title: n.title,
                artist_name: n.artist_name.unwrap_or_default(),
                album_title: n.album_title,
                artwork_url: n.artwork_url,
                duration_ms: n.duration_ms,
                isrc: None,
                is_in_library: true,
                source: RadioSource::Library,
                reason: format!("your listening · {} plays together", n.support_count),
                similarity_score: n.similarity_score,
                confidence: Some(n.confidence),
                candidate_in_degree_percentile: Some(n.candidate_in_degree_percentile),
                support_count: Some(n.support_count),
                primary_reason: n.primary_reason,
            })
            .collect();
    let engine = if blend == RadioBlend::Familiar {
        engine_results_from_track_similarity(db, recall_seed_id, limit, &exclude)
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let local_ids: Vec<i64> = tidal
        .iter()
        .chain(&lastfm)
        .map(|c| c.track_id)
        .filter(|id| *id > 0)
        .collect();
    let liked: HashSet<i64> = if local_ids.is_empty() {
        HashSet::new()
    } else {
        db.with_conn(|conn| {
            let placeholders = vec!["?"; local_ids.len()].join(", ");
            let mut stmt = conn.prepare(&format!(
                "SELECT id FROM tracks WHERE is_favorite = 1 AND id IN ({placeholders})"
            ))?;
            let ids = stmt
                .query_map(rusqlite::params_from_iter(local_ids.iter()), |row| {
                    row.get(0)
                })?
                .collect::<rusqlite::Result<HashSet<i64>>>()?;
            Ok(ids)
        })
        .unwrap_or_default()
    };
    let (taste, resolver) = build_taste_inputs(db, seed_track_id);
    let pool = HybridPool {
        tidal,
        lastfm,
        learned,
        engine,
    };
    let mut station = blend_hybrid(pool, &liked, &taste, &resolver, blend, limit);
    fill_library_isrcs(db, &mut station);
    station
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(source: RadioSource, id: i64, artist: &str, title: &str) -> RadioCandidate {
        RadioCandidate {
            track_id: if source == RadioSource::Lastfm { 0 } else { id },
            tidal_track_id: (source == RadioSource::Tidal).then_some(1000 + id),
            title: title.to_string(),
            artist_name: artist.to_string(),
            album_title: None,
            artwork_url: None,
            duration_ms: None,
            isrc: None,
            is_in_library: source == RadioSource::Library,
            source,
            reason: String::new(),
            similarity_score: 0.0,
            confidence: None,
            candidate_in_degree_percentile: None,
            support_count: None,
            primary_reason: None,
        }
    }

    fn titles(station: &[RadioCandidate]) -> Vec<&str> {
        station.iter().map(|c| c.title.as_str()).collect()
    }

    fn blend(pool: HybridPool, liked: &[i64], limit: usize) -> Vec<RadioCandidate> {
        blend_hybrid(
            pool,
            &liked.iter().copied().collect(),
            &TasteVector::default(),
            &ArtistResolver::default(),
            RadioBlend::Mixed,
            limit,
        )
    }

    #[test]
    fn a_pick_both_providers_list_rises_and_appears_once() {
        let pool = HybridPool {
            tidal: vec![
                pick(RadioSource::Tidal, 1, "A", "First"),
                pick(RadioSource::Tidal, 2, "B", "Second"),
                pick(RadioSource::Tidal, 3, "C", "Shared"),
            ],
            lastfm: vec![pick(RadioSource::Lastfm, 0, "C", "Shared")],
            ..Default::default()
        };
        let station = blend(pool, &[], 10);
        assert_eq!(titles(&station), vec!["Shared", "First", "Second"]);
        assert_eq!(
            station[0].source,
            RadioSource::Tidal,
            "TIDAL's playable row is kept"
        );
    }

    #[test]
    fn your_listening_and_likes_reorder_the_provider_pool() {
        let pool = HybridPool {
            tidal: vec![
                pick(RadioSource::Tidal, 1, "A", "Top"),
                pick(RadioSource::Tidal, 2, "B", "Loved and heard"),
                pick(RadioSource::Tidal, 3, "C", "Third"),
            ],
            learned: vec![pick(RadioSource::Library, 2, "B", "Loved and heard")],
            ..Default::default()
        };
        let station = blend(pool, &[2], 10);
        assert_eq!(titles(&station), vec!["Loved and heard", "Top", "Third"]);
    }

    #[test]
    fn library_adds_fill_a_capped_share_spread_through_the_station() {
        let tidal = (0..20)
            .map(|i| {
                pick(
                    RadioSource::Tidal,
                    i,
                    &format!("T{i}"),
                    &format!("Tidal {i}"),
                )
            })
            .collect();
        let learned = (100..120)
            .map(|i| {
                pick(
                    RadioSource::Library,
                    i,
                    &format!("L{i}"),
                    &format!("Library {i}"),
                )
            })
            .collect();
        let station = blend(
            HybridPool {
                tidal,
                learned,
                ..Default::default()
            },
            &[],
            20,
        );
        assert_eq!(station.len(), 20);
        let positions: Vec<usize> = station
            .iter()
            .enumerate()
            .filter(|(_, c)| c.source == RadioSource::Library)
            .map(|(i, _)| i)
            .collect();
        // Mixed gives the library a quarter of the station, not the tail.
        assert_eq!(positions.len(), 5);
        assert!(positions[0] < 5 && positions[4] < 20);
        assert!(positions.windows(2).all(|w| w[1] - w[0] >= 2));
    }

    #[test]
    fn without_providers_the_library_share_still_plays() {
        let learned = (0..3)
            .map(|i| {
                pick(
                    RadioSource::Library,
                    i,
                    &format!("L{i}"),
                    &format!("Library {i}"),
                )
            })
            .collect();
        let station = blend(
            HybridPool {
                learned,
                ..Default::default()
            },
            &[],
            20,
        );
        // Mixed caps library adds at 5 of 20; there are 3.
        assert_eq!(station.len(), 3);
    }
}
