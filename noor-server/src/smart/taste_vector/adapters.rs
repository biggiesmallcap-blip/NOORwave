//! Conversions from the existing taste representations into the canonical
//! [`TasteVector`]. Phase 1 only calls [`from_session_profile`] from the
//! automix path; the other two adapters are defined here so the contract is
//! visible in one place, but they're not yet consumed and will be tightened
//! when discovery and radio migrate.

use std::collections::HashSet;

use crate::db::models::{
    AnalyticsBehavior, AnalyticsGenreShare, AnalyticsOverview, AnalyticsTopArtist,
    ListenHistoryEntry,
};
use crate::playback::player::SessionTasteProfile;
use crate::smart::external_discovery::TasteMeshProfile;

use super::{AffinitySignal, SeedContext, TasteVector};

/// Bundle of analytics inputs that [`from_analytics_overview`] needs.
///
/// The original spec passed only `(overview, listen_history)` but
/// `AnalyticsOverview` lacks the per-artist and per-genre rollups that drive
/// affinity strength. Carrying the rollups explicitly keeps the call site
/// honest about what's being read.
#[allow(dead_code)] // Phase 2/3 — bundle is consumed when discovery or radio migrates.
pub struct AnalyticsContext<'a> {
    /// Phase 3 — useful for normalising affinity against library size and
    /// for deriving favourite-ratio biases. Not read by the placeholder
    /// implementation.
    pub overview: &'a AnalyticsOverview,
    pub behavior: &'a AnalyticsBehavior,
    pub top_artists: &'a [AnalyticsTopArtist],
    pub top_genres: &'a [AnalyticsGenreShare],
    pub recent_listens: &'a [ListenHistoryEntry],
}

/// Lossless wrap of an in-memory [`SessionTasteProfile`].
///
/// Walks the union of `positive_*` and `negative_*` keys so an artist or
/// genre with negative-only signal (one the user has only ever skipped)
/// still produces an `AffinitySignal`. Iterating just the positive map
/// would silently drop those.
///
/// Returns `(TasteVector, SeedContext)` because the source profile carries
/// the seed-track context too — pulling them apart at the boundary keeps
/// `TasteVector` reusable for non-automix consumers.
pub fn from_session_profile(profile: &SessionTasteProfile) -> (TasteVector, SeedContext) {
    let mut taste = TasteVector::default();

    let artist_ids: HashSet<i64> = profile
        .positive_artists
        .keys()
        .chain(profile.negative_artists.keys())
        .copied()
        .collect();
    for id in artist_ids {
        let pos = profile.positive_artists.get(&id).copied().unwrap_or(0.0);
        let neg = profile.negative_artists.get(&id).copied().unwrap_or(0.0);
        taste
            .artist_affinity
            .insert(id, AffinitySignal { pos, neg });
    }

    let genre_keys: HashSet<&str> = profile
        .positive_genres
        .keys()
        .chain(profile.negative_genres.keys())
        .map(String::as_str)
        .collect();
    for key in genre_keys {
        let pos = profile.positive_genres.get(key).copied().unwrap_or(0.0);
        let neg = profile.negative_genres.get(key).copied().unwrap_or(0.0);
        taste
            .genre_affinity
            .insert(key.to_string(), AffinitySignal { pos, neg });
    }

    taste.skipped_track_ids = profile.skipped_track_ids.clone();
    taste.recent_track_ids = profile.recent_track_ids.clone();
    // exploration_bias / energy_pref / bpm_pref stay None — SessionTasteProfile
    // has no aggregate fatigue or DSP-preference signal to draw from.

    let seed = SeedContext {
        artist_id: profile.current_artist_id,
        album_id: profile.current_album_id,
        source: profile.current_source.clone(),
        genres: profile.current_genres.clone(),
    };

    (taste, seed)
}

/// Convert from the discovery-side [`TasteMeshProfile`].
///
/// **Phase 2/3 follow-up.** Defined now so the contract is visible but not
/// called by the Phase 1 automix migration. Two known gaps:
///
/// 1. `TasteMeshProfile.artist_affinity` is keyed by lowercased artist
///    *name* whereas `TasteVector.artist_affinity` is keyed by `artist_id`.
///    There is no name → id resolver at this layer, so the artist map ends
///    up empty. Discovery will need to plumb ids through when it migrates.
/// 2. `TasteMeshProfile` carries only positive signal, so every produced
///    `AffinitySignal` has `neg = 0.0`. A real negative signal will need to
///    be derived from skip rollups when discovery actually consumes this.
#[allow(dead_code)] // Phase 2/3 — wired up when discovery migrates.
pub fn from_taste_mesh(mesh: &TasteMeshProfile) -> TasteVector {
    let mut taste = TasteVector::default();

    for (name, weight) in &mesh.genre_affinity {
        taste.genre_affinity.insert(
            name.clone(),
            AffinitySignal {
                pos: *weight,
                neg: 0.0,
            },
        );
    }

    // TasteMesh's novelty_bias lives in [1.0, 2.0] meaning "fatigue
    // intensity" — high values mean the user has been skipping a lot.
    // TasteVector's exploration_bias lives in [0.0, 1.0] meaning "wants
    // new". High fatigue maps to high desire for novelty, so subtracting
    // 1.0 lines them up (and the clamp protects against any out-of-range
    // input).
    taste.exploration_bias = Some((mesh.novelty_bias as f32 - 1.0).clamp(0.0, 1.0));

    taste
}

/// Build a [`TasteVector`] directly from analytics rollups.
///
/// **Phase 2/3 follow-up.** Defined now so the contract is visible but not
/// called by the Phase 1 automix migration.
///
/// Genre `neg` is always `0.0` because `AnalyticsGenreShare` carries no
/// per-genre skip counter. A real negative signal will need a new query
/// (per-genre skip rollup) when this adapter is actually consumed.
#[allow(dead_code)] // Phase 2/3 — wired up when radio or discovery migrates.
pub fn from_analytics_overview(ctx: &AnalyticsContext) -> TasteVector {
    let mut taste = TasteVector::default();

    for artist in ctx.top_artists {
        let completed = artist.completed_listens.max(0) as f64;
        let total = artist.listens.max(0) as f64;
        let skipped = (total - completed).max(0.0);
        taste.artist_affinity.insert(
            artist.artist_id,
            AffinitySignal {
                pos: completed,
                neg: skipped,
            },
        );
    }

    for genre in ctx.top_genres {
        // Lowercase to match the key shape `automix_score` looks up.
        let key = genre.genre_name.trim().to_ascii_lowercase();
        taste.genre_affinity.insert(
            key,
            AffinitySignal {
                pos: genre.listens.max(0) as f64,
                neg: 0.0,
            },
        );
    }

    for listen in ctx.recent_listens {
        taste.recent_track_ids.insert(listen.track_id);
        if !listen.completed {
            taste.skipped_track_ids.insert(listen.track_id);
        }
    }

    if ctx.behavior.total_listens > 0 {
        let skip_rate = ctx.behavior.skipped_listens as f32 / ctx.behavior.total_listens as f32;
        taste.exploration_bias = Some(skip_rate.clamp(0.0, 1.0));
    }

    taste
}
