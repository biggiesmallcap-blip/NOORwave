//! Canonical taste representation shared across discovery, radio, and
//! automix. Phase 1 introduces the type and migrates only automix to consume
//! it; discovery and radio still build their own shapes and convert at the
//! boundary via the adapters in [`adapters`]. Phase 3 will collapse the
//! remaining producers.
//!
//! `AffinitySignal` is a lossless wrap of the existing positive/negative
//! signal pairs that `SessionTasteProfile` already carries. Adapter sites do
//! no normalisation, clamping, or division — scoring sites apply their own
//! coefficients. This is what lets the parity gate hold byte-identical
//! ranking across the migration.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub mod adapters;

/// Per-key positive/negative signal volume. Direction and magnitude are kept
/// separate so scoring sites can apply asymmetric coefficients (e.g. the
/// historical "negatives hurt more than positives help" weighting) without
/// the adapter having to bake the asymmetry in.
///
/// Both fields are `f64` to match `SessionTasteProfile`'s existing
/// `HashMap<i64, f64>` exactly — there is no precision round-trip on the
/// adapter path.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AffinitySignal {
    pub pos: f64,
    pub neg: f64,
}

/// What the user appears to like, in a shape every consumer can read.
///
/// Artists are keyed by `artist_id` because both the listen-history-walking
/// path (`SessionTasteProfile`) and the analytics rollup
/// (`AnalyticsTopArtist`) carry the id losslessly. Genres are keyed by the
/// lowercased name produced by [`crate::playback::player`]'s genre
/// normalisation — `AnalyticsGenreShare` has no `genre_id` to use, and
/// switching to a name key everywhere is the honest representation of the
/// data we have.
#[derive(Debug, Clone)]
pub struct TasteVector {
    pub artist_affinity: HashMap<i64, AffinitySignal>,
    pub genre_affinity: HashMap<String, AffinitySignal>,

    /// Hard suppression set — automix multiplies the score by 0.1 for tracks
    /// in here.
    pub skipped_track_ids: HashSet<i64>,
    /// Soft penalty set — recent listens; consumers may down-weight or
    /// avoid these.
    pub recent_track_ids: HashSet<i64>,

    /// Aggregate biases. `None` means the producing adapter had no signal
    /// for this dimension; consumers should treat that as "no preference"
    /// rather than zero. Keeps adapters from lying about absent data.
    pub exploration_bias: Option<f32>,
    /// Phase 3 — populated when adapters start deriving DSP-preference
    /// signals. Consumers ignore until then.
    #[allow(dead_code)]
    pub energy_pref: Option<f32>,
    #[allow(dead_code)]
    pub bpm_pref: Option<f32>,

    /// Build timestamp for cache invalidation by callers that hold the
    /// vector across requests. Phase 1 builds and discards immediately.
    #[allow(dead_code)]
    pub built_at: Instant,
}

impl Default for TasteVector {
    fn default() -> Self {
        Self {
            artist_affinity: HashMap::new(),
            genre_affinity: HashMap::new(),
            skipped_track_ids: HashSet::new(),
            recent_track_ids: HashSet::new(),
            exploration_bias: None,
            energy_pref: None,
            bpm_pref: None,
            built_at: Instant::now(),
        }
    }
}

/// Per-query "currently playing track" snapshot.
///
/// Lives next to `TasteVector` rather than inside it because automix is the
/// only consumer that has a seed track. Discovery and radio compute taste
/// without one. Keeping the seed concept out of `TasteVector` means those
/// consumers don't have to carry empty/None fields they will never set.
///
/// If a third call site outside automix ever wants this, the abstraction is
/// wrong and we should reconsider before extending.
#[derive(Debug, Clone, Default)]
pub struct SeedContext {
    pub artist_id: Option<i64>,
    /// Carried lossless from `SessionTasteProfile.current_album_id`; the
    /// historical `automix_score` never read it, but keeping it on the
    /// snapshot makes the Phase 3 cleanup honest about what data actually
    /// flowed through the old path.
    #[allow(dead_code)]
    pub album_id: Option<i64>,
    pub source: Option<String>,
    /// Genre keys, lowercased via the same normaliser the scoring path uses.
    pub genres: HashSet<String>,
}
