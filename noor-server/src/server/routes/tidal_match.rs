//! Matching a pending (artist, title) row to the right TIDAL track: scoring, version descriptors (live, remix, remaster), and metadata import.

use super::*;

pub(super) const MATCH_QUALITY_THRESHOLD: f64 = 0.85;

pub(super) const RESOLVER_POOL_SIZE: usize = 4;

pub(super) const SCORE_W_ARTIST: f64 = 0.60;

pub(super) const SCORE_W_TITLE: f64 = 0.40;

/// Lowercase, alphanumerics only, single spaces: "AC/DC" -> "ac dc".
fn normalize_match_text(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn score_tidal_candidate(
    result_artist: &str,
    result_title: &str,
    pending_artist: &str,
    pending_title: &str,
) -> f64 {
    let a = strsim::jaro_winkler(
        &normalize_match_text(result_artist),
        &normalize_match_text(pending_artist),
    );
    let t = strsim::jaro_winkler(
        &normalize_match_text(result_title),
        &normalize_match_text(pending_title),
    );
    SCORE_W_ARTIST * a + SCORE_W_TITLE * t
}

/// Whole-name Jaro-Winkler that still reads as the same artist written
/// differently (an accent: 0.943). Crystal Lake vs Crystal Voices scores 0.879
/// and must fail.
pub(super) const ARTIST_IDENTITY_MIN: f64 = 0.93;

/// The result is by the requested artist: the same name (ignoring "the" and
/// "and"), a near-identical spelling, or one credit extending the other at a
/// word boundary ("Crystal Voices - Deborah Van Dyke & Valerie Farnsworth").
/// Checked on its own because the 60/40 artist/title blend let a matching
/// title carry a different artist past the threshold.
pub(super) fn artist_identity_matches(pending_artist: &str, result_artist: &str) -> bool {
    fn key(s: &str) -> String {
        let normalized = normalize_match_text(s);
        let words: Vec<&str> = normalized
            .split(' ')
            .filter(|w| !w.is_empty() && *w != "the" && *w != "and")
            .collect();
        if words.is_empty() {
            normalized
        } else {
            words.join(" ")
        }
    }
    let pending = key(pending_artist);
    let result = key(result_artist);
    if pending.is_empty() || result.is_empty() {
        return false;
    }
    if pending == result {
        return true;
    }
    let (short, long) = if pending.len() <= result.len() {
        (&pending, &result)
    } else {
        (&result, &pending)
    };
    if long.starts_with(short.as_str()) && long[short.len()..].starts_with(' ') {
        return true;
    }
    strsim::jaro_winkler(&pending, &result) >= ARTIST_IDENTITY_MIN
}

pub(crate) fn import_metadata_from_search_track(
    t: TidalSearchTrack,
) -> tidal_import::ImportTrackMetadata {
    tidal_import::ImportTrackMetadata {
        tidal_id: t.id,
        title: t.title,
        artist_name: t.artist_name.unwrap_or_default(),
        artist_tidal_id: t.artist_id,
        artist_picture: t.artist_picture,
        album_title: t.album_title,
        album_tidal_id: t.album_id,
        album_artwork_url: t.artwork_url,
        duration_ms: Some(t.duration * 1000),
    }
}

pub(crate) fn import_metadata_from_tidal_track(t: TidalTrack) -> tidal_import::ImportTrackMetadata {
    let album_title = t.album.as_ref().map(|album| album.title.clone());
    let album_tidal_id = t.album.as_ref().map(|album| album.id);
    let album_artwork_url = t
        .album
        .as_ref()
        .and_then(|album| TidalClient::get_artwork_url(&album.cover, 640));
    tidal_import::ImportTrackMetadata {
        tidal_id: t.id,
        title: t.title,
        artist_name: t.artist.name,
        artist_tidal_id: Some(t.artist.id),
        // Artist photos come in 160/320/480/750; 640 is a cover size.
        artist_picture: TidalClient::get_artwork_url(&t.artist.picture, 750),
        album_title,
        album_tidal_id,
        album_artwork_url,
        duration_ms: Some(t.duration * 1000),
    }
}

pub(crate) const TIDAL_RESOLVE_POOL: i32 = 10;

/// How a TIDAL track relates to the plain studio recording, inferred from its
/// `version` field (authoritative) or a trailing descriptor in the title.
///
/// `Original` is the canonical performance: no version tag, or a marker that
/// only describes mastering/format (remaster, mono, deluxe edition...) which is
/// the *same* recording and stays eligible. Every other class is a different
/// recording and gets demoted unless the request explicitly asked for it. This
/// version axis is the only thing separating "American Pie" from "American Pie
/// (L'Tric Remix)": they share a base title and artist, so title+artist scoring
/// alone ties them at 1.0 and the remix can win by listing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VersionClass {
    Original,
    Remix,
    Live,
    Acoustic,
    Instrumental,
    Cover,
    SpedSlowed,
    Edit,
    OtherVariant,
}

pub(super) fn normalize_version_text(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn version_text_has_word(normalized: &str, word: &str) -> bool {
    normalized.split(' ').any(|tok| tok == word)
}

/// Classify a free-text version descriptor. Returns `None` when nothing is
/// recognized, so callers decide what an unknown tag means in context: a
/// populated TIDAL `version` field is a deliberate variant flag, while a bare
/// title parenthetical like "(Pt. 1)" is probably just part of the title.
pub(super) fn classify_version_descriptor(descriptor: &str) -> Option<VersionClass> {
    let d = normalize_version_text(descriptor);
    if d.is_empty() {
        return None;
    }
    // Mastering / format / explicitly-original markers describe the same
    // performance, so they resolve to Original. Checked first so "Original Mix"
    // and "Deluxe Edition" never fall through to the "mix"/"edit" branches.
    const MASTERING: &[&str] = &[
        "original mix",
        "original version",
        "album version",
        "single version",
        "original",
        "remaster",
        "remastered",
        "mono",
        "stereo",
        "deluxe",
        "anniversary",
        "expanded",
        "reissue",
        "edition",
        "bonus",
    ];
    if MASTERING.iter().any(|m| d.contains(m)) {
        return Some(VersionClass::Original);
    }
    const REMIX: &[&str] = &[
        "remix", "rmx", "bootleg", "rework", "flip", "vip", "mashup", "mash up", "club mix", "dub",
    ];
    if REMIX.iter().any(|m| d.contains(m)) {
        return Some(VersionClass::Remix);
    }
    if version_text_has_word(&d, "live") || d.contains("in concert") {
        return Some(VersionClass::Live);
    }
    if d.contains("acoustic") || d.contains("unplugged") {
        return Some(VersionClass::Acoustic);
    }
    if d.contains("instrumental") || d.contains("karaoke") {
        return Some(VersionClass::Instrumental);
    }
    if d.contains("cover")
        || d.contains("originally performed")
        || d.contains("made famous")
        || d.contains("tribute")
    {
        return Some(VersionClass::Cover);
    }
    if d.contains("sped up")
        || d.contains("spedup")
        || d.contains("slowed")
        || d.contains("nightcore")
    {
        return Some(VersionClass::SpedSlowed);
    }
    if version_text_has_word(&d, "edit") || d.contains("extended") {
        return Some(VersionClass::Edit);
    }
    None
}

/// Split a trailing variant descriptor off a title. Only the last bracketed
/// group or a " - " tail is considered:
/// "American Pie (L'Tric Remix)" -> ("American Pie", Some("L'Tric Remix")).
pub(super) fn split_title_descriptor(title: &str) -> (String, Option<String>) {
    let t = title.trim();
    if let Some(open) = t.rfind(['(', '[']) {
        let want_close = if t.as_bytes()[open] == b'(' {
            b')'
        } else {
            b']'
        };
        if t.as_bytes().last() == Some(&want_close) {
            let inner = t[open + 1..t.len() - 1].trim();
            let base = t[..open].trim();
            if !inner.is_empty() && !base.is_empty() {
                return (base.to_string(), Some(inner.to_string()));
            }
        }
    }
    if let Some(idx) = t.rfind(" - ") {
        let desc = t[idx + 3..].trim();
        let base = t[..idx].trim();
        if !desc.is_empty() && !base.is_empty() {
            return (base.to_string(), Some(desc.to_string()));
        }
    }
    (t.to_string(), None)
}

/// Base title (for fuzzy scoring) plus version class for a search candidate. The
/// `version` field wins; a populated-but-unrecognized version still means "not
/// the plain original" and demotes. Without a version field we read a title
/// descriptor, where an unrecognized parenthetical is kept as part of the title.
pub(super) fn classify_candidate(track: &TidalSearchTrack) -> (String, VersionClass) {
    let version = track
        .extra
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(version) = version {
        let class = classify_version_descriptor(version).unwrap_or(VersionClass::OtherVariant);
        return (track.title.clone(), class);
    }
    classify_title_field(&track.title)
}

/// Base title plus version class for a free title string with no separate
/// version field (e.g. a Last.fm suggestion). Unrecognized descriptors stay
/// Original so genuine titles like "Shine On You Crazy Diamond (Pt. 1)" match.
pub(super) fn classify_title_field(title: &str) -> (String, VersionClass) {
    let (base, desc) = split_title_descriptor(title);
    match desc.as_deref().and_then(classify_version_descriptor) {
        Some(class) => (base, class),
        None => (title.trim().to_string(), VersionClass::Original),
    }
}

pub(super) fn version_quality_rank(quality: Option<&str>) -> u8 {
    match quality.map(str::to_ascii_uppercase).as_deref() {
        Some("HI_RES_LOSSLESS") | Some("HI_RES") => 3,
        Some("LOSSLESS") => 2,
        Some("HIGH") => 1,
        _ => 0,
    }
}

/// Pick the best TIDAL search result for a pending `(artist, title)`, preferring
/// the version the request actually implies. Pure (no network) so it can be unit
/// tested against synthetic candidate sets.
///
/// 1. Score every candidate on base-title + artist Jaro-Winkler (variant
///    descriptors stripped first), keeping those that clear the threshold.
/// 2. Partition by whether the candidate's version class matches the request.
///    Prefer the matching set; fall back to the rest only when nothing matched,
///    so a song that exists *only* as a remix still resolves instead of stalling.
/// 3. Within the chosen set, rank by score, then descriptor closeness when a
///    specific variant was named (so a named remix beats a different one), then
///    audio quality as a hi-fi-friendly final tiebreak.
pub(crate) fn select_best_tidal_match(
    pending_artist: &str,
    pending_title: &str,
    results: Vec<TidalSearchTrack>,
) -> Option<(f64, TidalSearchTrack)> {
    let (pending_base, pending_class) = classify_title_field(pending_title);
    let pending_desc_norm = split_title_descriptor(pending_title)
        .1
        .map(|d| normalize_version_text(&d))
        .unwrap_or_default();

    struct Scored {
        score: f64,
        class: VersionClass,
        desc_sim: f64,
        quality: u8,
        track: TidalSearchTrack,
    }

    let mut scored: Vec<Scored> = results
        .into_iter()
        .filter_map(|track| {
            let (cand_base, class) = classify_candidate(&track);
            let score = score_tidal_candidate(
                track.artist_name.as_deref().unwrap_or(""),
                &cand_base,
                pending_artist,
                &pending_base,
            );
            if score < MATCH_QUALITY_THRESHOLD {
                return None;
            }
            let cand_desc_norm = track
                .extra
                .get("version")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| split_title_descriptor(&track.title).1)
                .map(|d| normalize_version_text(&d))
                .unwrap_or_default();
            let desc_sim = if pending_desc_norm.is_empty() {
                0.0
            } else {
                strsim::jaro_winkler(&pending_desc_norm, &cand_desc_norm)
            };
            Some(Scored {
                score,
                class,
                desc_sim,
                quality: version_quality_rank(track.audio_quality.as_deref()),
                track,
            })
        })
        .collect();

    if scored.is_empty() {
        return None;
    }

    // Prefer candidates whose version class matches the request; only keep the
    // mismatched ones if nothing matched at all (the fallback).
    if scored
        .iter()
        .any(|s| version_intent_matches(pending_class, s.class))
    {
        scored.retain(|s| version_intent_matches(pending_class, s.class));
    }

    let want_desc = !pending_desc_norm.is_empty();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                if want_desc {
                    b.desc_sim
                        .partial_cmp(&a.desc_sim)
                        .unwrap_or(std::cmp::Ordering::Equal)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then(b.quality.cmp(&a.quality))
    });

    scored.into_iter().next().map(|s| (s.score, s.track))
}

/// A candidate satisfies the request when its version class is the same. A clean
/// request (`Original`) only accepts originals; a request that named a variant
/// (remix, acoustic...) only accepts that same kind.
pub(super) fn version_intent_matches(pending: VersionClass, candidate: VersionClass) -> bool {
    pending == candidate
}

#[cfg(test)]
mod artist_identity_tests {
    use super::artist_identity_matches;

    #[test]
    fn different_artists_sharing_a_word_do_not_match() {
        assert!(!artist_identity_matches("Crystal Voices", "Crystal Lake"));
        assert!(!artist_identity_matches(
            "David Hickey",
            "David Kuckhermann"
        ));
        assert!(!artist_identity_matches("Don McLean", "Madonna"));
    }

    #[test]
    fn same_artist_written_differently_matches() {
        assert!(artist_identity_matches("Beyonce", "Beyonc\u{e9}"));
        assert!(artist_identity_matches("Sigur Ros", "Sigur R\u{f3}s"));
        assert!(artist_identity_matches("Jay-Z", "JAY Z"));
        assert!(artist_identity_matches("The Weeknd", "Weeknd"));
        assert!(artist_identity_matches(
            "Simon & Garfunkel",
            "Simon and Garfunkel"
        ));
    }

    #[test]
    fn extended_credit_matches_at_a_word_boundary() {
        assert!(artist_identity_matches(
            "Crystal Voices",
            "Crystal Voices - Deborah Van Dyke & Valerie Farnsworth"
        ));
        assert!(artist_identity_matches(
            "Calvin Harris, Rihanna",
            "Calvin Harris"
        ));
        // "Ash" is a prefix of "Ashanti" but not at a word boundary (JW 0.87).
        assert!(!artist_identity_matches("Ash", "Ashanti"));
    }

    #[test]
    fn empty_artist_never_matches() {
        assert!(!artist_identity_matches("", "Crystal Voices"));
        assert!(!artist_identity_matches("Crystal Voices", ""));
    }
}
