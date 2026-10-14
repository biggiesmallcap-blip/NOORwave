//! Cut classification: the kind of every catalog video (official, live,
//! session, lyric, visualizer, remix, dance, interview, trailer, clip,
//! longform) from multilingual title markers, TIDAL type and duration, plus
//! the one shared `programmable` rule every surface obeys. Writes video_facets.
//!
//! TIDAL's own type is nearly useless here ("Live" on 380 rows while titles
//! say live on thousands, a third of them only in Spanish, Portuguese or
//! French), so titles decide and the type only breaks ties. Markers are
//! matched as whole words on an accent-folded, lowercased title. Weak words
//! ("live", "session", "edit", "version") only count in the title's suffix
//! (brackets or after a dash), so a song called "Live Forever" or "Trailer
//! Park" stays an official video; strong phrases ("en vivo", "behind the
//! scenes", "lyric video") count anywhere.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::services::video_discovery::names::name_key;
use crate::services::video_radio::video_song_key;
use crate::services::video_sets::VideoCandidate;

/// TIDAL's "Various Artists": compilations, never programmable.
pub const VARIOUS_ARTISTS_ID: i64 = 2935;
/// Anything shorter is a clip, whatever its title says.
pub const CLIP_UNDER_S: i64 = 90;
/// Anything this long without a live or session marker is long form.
pub const LONGFORM_FROM_S: i64 = 1800;

/// The kind of a video. Serialized names match the frontend `VideoCut`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cut {
    Official,
    Live,
    Session,
    Lyric,
    Visualizer,
    Remix,
    Dance,
    Interview,
    Trailer,
    Clip,
    Longform,
}

impl Cut {
    /// Every cut, in bit order (`bit()` is `1 << index`).
    pub const ALL: [Cut; 11] = [
        Cut::Official,
        Cut::Live,
        Cut::Session,
        Cut::Lyric,
        Cut::Visualizer,
        Cut::Remix,
        Cut::Dance,
        Cut::Interview,
        Cut::Trailer,
        Cut::Clip,
        Cut::Longform,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Cut::Official => "official",
            Cut::Live => "live",
            Cut::Session => "session",
            Cut::Lyric => "lyric",
            Cut::Visualizer => "visualizer",
            Cut::Remix => "remix",
            Cut::Dance => "dance",
            Cut::Interview => "interview",
            Cut::Trailer => "trailer",
            Cut::Clip => "clip",
            Cut::Longform => "longform",
        }
    }

    pub fn parse(raw: &str) -> Option<Cut> {
        Cut::ALL.into_iter().find(|cut| cut.as_str() == raw)
    }

    /// Human label for version lists ("Live", "Lyric video").
    pub fn label(self) -> &'static str {
        match self {
            Cut::Official => "Official",
            Cut::Live => "Live",
            Cut::Session => "Session",
            Cut::Lyric => "Lyric video",
            Cut::Visualizer => "Visualizer",
            Cut::Remix => "Remix",
            Cut::Dance => "Dance",
            Cut::Interview => "Interview",
            Cut::Trailer => "Trailer",
            Cut::Clip => "Clip",
            Cut::Longform => "Long form",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// This cut's bit in `video_songs.cuts_mask`.
    pub fn bit(self) -> i64 {
        1 << self.index()
    }

    /// Cuts present in a `cuts_mask`, in bit order.
    pub fn from_mask(mask: i64) -> Vec<Cut> {
        Cut::ALL
            .into_iter()
            .filter(|cut| mask & cut.bit() != 0)
            .collect()
    }

    /// Whether a channel, radio or shelf may ever play this kind of video.
    pub fn is_programmable(self) -> bool {
        !matches!(self, Cut::Interview | Cut::Trailer | Cut::Clip)
    }

    /// Which cut leads a song when nothing else decides: the official video,
    /// then live takes, then the rest. Lower is better.
    pub fn best_rank(self) -> u8 {
        match self {
            Cut::Official => 0,
            Cut::Live => 1,
            Cut::Session => 2,
            Cut::Remix => 3,
            Cut::Lyric => 4,
            Cut::Visualizer => 5,
            Cut::Dance => 6,
            Cut::Longform => 7,
            Cut::Clip => 8,
            Cut::Trailer => 9,
            Cut::Interview => 10,
        }
    }
}

// ---------------------------------------------------------------- markers

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Interview,
    /// Interview only when no official-video marker says otherwise
    /// ("La Mision (Episodio 2) [Official Video]" is a music video).
    WeakInterview,
    Trailer,
    Dance,
    Lyric,
    Visualizer,
    /// Session formats that win over a live marker ("Live Session").
    SessionStrong,
    Live,
    SessionWeak,
    Remix,
    Official,
}

impl Kind {
    fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Anywhere,
    Suffix,
}

struct Marker {
    words: Vec<&'static str>,
    kind: Kind,
    scope: Scope,
    /// The marker does not count right after this word ("SUGA of BTS",
    /// "Original Mix").
    unless_after: Option<&'static str>,
}

/// (phrase, kind, scope, unless_after). Phrases are folded: lowercase, no
/// accents, punctuation as spaces ("Q&A" -> "q a", "Derriere" for
/// "Derriere").
const MARKERS: &[(&str, Kind, Scope, Option<&str>)] = &[
    // Interviews, documentaries, behind the scenes.
    ("interview", Kind::Interview, Scope::Anywhere, None),
    ("interviews", Kind::Interview, Scope::Anywhere, None),
    ("entrevista", Kind::Interview, Scope::Anywhere, None),
    ("entrevue", Kind::Interview, Scope::Anywhere, None),
    ("intervista", Kind::Interview, Scope::Anywhere, None),
    ("behind the scenes", Kind::Interview, Scope::Anywhere, None),
    ("behind the scene", Kind::Interview, Scope::Anywhere, None),
    ("making of", Kind::Interview, Scope::Anywhere, None),
    ("making off", Kind::Interview, Scope::Anywhere, None),
    ("making the record", Kind::Interview, Scope::Anywhere, None),
    ("making the album", Kind::Interview, Scope::Anywhere, None),
    ("making the video", Kind::Interview, Scope::Anywhere, None),
    ("documentary", Kind::Interview, Scope::Anywhere, None),
    ("documental", Kind::Interview, Scope::Anywhere, None),
    ("documentaire", Kind::Interview, Scope::Anywhere, None),
    ("documentario", Kind::Interview, Scope::Anywhere, None),
    ("dokumentation", Kind::Interview, Scope::Anywhere, None),
    ("commentary", Kind::Interview, Scope::Anywhere, None),
    ("track by track", Kind::Interview, Scope::Anywhere, None),
    ("tour diary", Kind::Interview, Scope::Anywhere, None),
    ("studio diary", Kind::Interview, Scope::Anywhere, None),
    ("vlog", Kind::Interview, Scope::Anywhere, None),
    ("in conversation", Kind::Interview, Scope::Anywhere, None),
    ("behind the video", Kind::Interview, Scope::Anywhere, None),
    ("behind the song", Kind::Interview, Scope::Anywhere, None),
    ("story behind", Kind::Interview, Scope::Anywhere, None),
    ("q a", Kind::Interview, Scope::Anywhere, None),
    ("unboxing", Kind::Interview, Scope::Anywhere, None),
    (
        "derriere les coulisses",
        Kind::Interview,
        Scope::Anywhere,
        None,
    ),
    ("les coulisses", Kind::Interview, Scope::Anywhere, None),
    ("detras de camaras", Kind::Interview, Scope::Anywhere, None),
    ("detras de camara", Kind::Interview, Scope::Anywhere, None),
    ("detras de escena", Kind::Interview, Scope::Anywhere, None),
    (
        "por tras das cameras",
        Kind::Interview,
        Scope::Anywhere,
        None,
    ),
    ("bastidores", Kind::Interview, Scope::Anywhere, None),
    ("dietro le quinte", Kind::Interview, Scope::Anywhere, None),
    (
        "hinter den kulissen",
        Kind::Interview,
        Scope::Anywhere,
        None,
    ),
    ("bts", Kind::Interview, Scope::Suffix, Some("of")),
    ("episode", Kind::WeakInterview, Scope::Anywhere, None),
    ("episodes", Kind::WeakInterview, Scope::Anywhere, None),
    ("episodio", Kind::WeakInterview, Scope::Anywhere, None),
    ("webisode", Kind::WeakInterview, Scope::Anywhere, None),
    ("reaction", Kind::WeakInterview, Scope::Suffix, None),
    // Trailers and teasers.
    ("bande annonce", Kind::Trailer, Scope::Anywhere, None),
    ("sneak peek", Kind::Trailer, Scope::Anywhere, None),
    ("album preview", Kind::Trailer, Scope::Anywhere, None),
    ("official trailer", Kind::Trailer, Scope::Anywhere, None),
    ("official teaser", Kind::Trailer, Scope::Anywhere, None),
    ("trailer oficial", Kind::Trailer, Scope::Anywhere, None),
    ("teaser oficial", Kind::Trailer, Scope::Anywhere, None),
    ("trailer", Kind::Trailer, Scope::Suffix, None),
    ("teaser", Kind::Trailer, Scope::Suffix, None),
    ("coming soon", Kind::Trailer, Scope::Suffix, None),
    ("announcement", Kind::Trailer, Scope::Suffix, None),
    ("snippet", Kind::Trailer, Scope::Suffix, None),
    // Dance practice and vertical cuts.
    ("dance practice", Kind::Dance, Scope::Anywhere, None),
    ("choreography", Kind::Dance, Scope::Anywhere, None),
    ("choreo", Kind::Dance, Scope::Anywhere, None),
    ("coreografia", Kind::Dance, Scope::Anywhere, None),
    ("choregraphie", Kind::Dance, Scope::Anywhere, None),
    ("dance video", Kind::Dance, Scope::Anywhere, None),
    ("relay dance", Kind::Dance, Scope::Anywhere, None),
    ("fancam", Kind::Dance, Scope::Anywhere, None),
    ("focus cam", Kind::Dance, Scope::Anywhere, None),
    ("vertical video", Kind::Dance, Scope::Anywhere, None),
    ("vertical", Kind::Dance, Scope::Suffix, None),
    // Lyric videos.
    ("lyric video", Kind::Lyric, Scope::Anywhere, None),
    ("lyrics video", Kind::Lyric, Scope::Anywhere, None),
    ("video lyric", Kind::Lyric, Scope::Anywhere, None),
    ("video lyrics", Kind::Lyric, Scope::Anywhere, None),
    ("official lyric", Kind::Lyric, Scope::Anywhere, None),
    ("official lyrics", Kind::Lyric, Scope::Anywhere, None),
    ("video letra", Kind::Lyric, Scope::Anywhere, None),
    ("letra oficial", Kind::Lyric, Scope::Anywhere, None),
    ("karaoke", Kind::Lyric, Scope::Anywhere, None),
    ("lyric", Kind::Lyric, Scope::Suffix, None),
    ("lyrics", Kind::Lyric, Scope::Suffix, None),
    ("letra", Kind::Lyric, Scope::Suffix, None),
    ("letras", Kind::Lyric, Scope::Suffix, None),
    ("con letra", Kind::Lyric, Scope::Suffix, None),
    ("paroles", Kind::Lyric, Scope::Suffix, None),
    ("testo", Kind::Lyric, Scope::Suffix, None),
    ("liedtext", Kind::Lyric, Scope::Suffix, None),
    ("lirik", Kind::Lyric, Scope::Suffix, None),
    // Visualizers and audio-only uploads.
    ("visualizer", Kind::Visualizer, Scope::Anywhere, None),
    ("visualiser", Kind::Visualizer, Scope::Anywhere, None),
    ("visualizador", Kind::Visualizer, Scope::Anywhere, None),
    ("official audio", Kind::Visualizer, Scope::Anywhere, None),
    ("audio oficial", Kind::Visualizer, Scope::Anywhere, None),
    ("audio officiel", Kind::Visualizer, Scope::Anywhere, None),
    ("audio ufficiale", Kind::Visualizer, Scope::Anywhere, None),
    ("offizielles audio", Kind::Visualizer, Scope::Anywhere, None),
    ("art track", Kind::Visualizer, Scope::Anywhere, None),
    ("static video", Kind::Visualizer, Scope::Anywhere, None),
    ("official preview", Kind::Visualizer, Scope::Anywhere, None),
    ("audio only", Kind::Visualizer, Scope::Anywhere, None),
    ("audio", Kind::Visualizer, Scope::Suffix, None),
    // Session formats that beat a live marker.
    ("tiny desk", Kind::SessionStrong, Scope::Anywhere, None),
    ("live session", Kind::SessionStrong, Scope::Anywhere, None),
    ("live sessions", Kind::SessionStrong, Scope::Anywhere, None),
    ("session live", Kind::SessionStrong, Scope::Anywhere, None),
    ("sessions live", Kind::SessionStrong, Scope::Anywhere, None),
    (
        "acoustic session",
        Kind::SessionStrong,
        Scope::Anywhere,
        None,
    ),
    (
        "acoustic sessions",
        Kind::SessionStrong,
        Scope::Anywhere,
        None,
    ),
    ("studio session", Kind::SessionStrong, Scope::Anywhere, None),
    ("live lounge", Kind::SessionStrong, Scope::Anywhere, None),
    ("colors show", Kind::SessionStrong, Scope::Anywhere, None),
    ("like a version", Kind::SessionStrong, Scope::Anywhere, None),
    ("take away show", Kind::SessionStrong, Scope::Anywhere, None),
    ("vevo dscvr", Kind::SessionStrong, Scope::Anywhere, None),
    ("vevo lift", Kind::SessionStrong, Scope::Anywhere, None),
    ("sofar sounds", Kind::SessionStrong, Scope::Anywhere, None),
    ("audiotree", Kind::SessionStrong, Scope::Anywhere, None),
    (
        "mahogany session",
        Kind::SessionStrong,
        Scope::Anywhere,
        None,
    ),
    ("la blogotheque", Kind::SessionStrong, Scope::Anywhere, None),
    ("kexp", Kind::SessionStrong, Scope::Anywhere, None),
    // Live: concerts, festivals, "en vivo", "ao vivo".
    ("en vivo", Kind::Live, Scope::Anywhere, None),
    ("ao vivo", Kind::Live, Scope::Anywhere, None),
    ("en directo", Kind::Live, Scope::Anywhere, None),
    ("en concierto", Kind::Live, Scope::Anywhere, None),
    ("en concert", Kind::Live, Scope::Anywhere, None),
    ("dal vivo", Kind::Live, Scope::Anywhere, None),
    ("live at", Kind::Live, Scope::Anywhere, None),
    ("live from", Kind::Live, Scope::Anywhere, None),
    ("in concert", Kind::Live, Scope::Anywhere, None),
    ("unplugged", Kind::Live, Scope::Anywhere, None),
    ("live", Kind::Live, Scope::Suffix, None),
    ("concert", Kind::Live, Scope::Suffix, None),
    ("concierto", Kind::Live, Scope::Suffix, None),
    ("konzert", Kind::Live, Scope::Suffix, None),
    ("vivo", Kind::Live, Scope::Suffix, None),
    ("directo", Kind::Live, Scope::Suffix, None),
    ("tour", Kind::Live, Scope::Suffix, None),
    ("festival", Kind::Live, Scope::Suffix, None),
    ("dvd", Kind::Live, Scope::Suffix, None),
    // Plain session words.
    ("session", Kind::SessionWeak, Scope::Suffix, None),
    ("sessions", Kind::SessionWeak, Scope::Suffix, None),
    ("acoustic", Kind::SessionWeak, Scope::Suffix, None),
    ("acustico", Kind::SessionWeak, Scope::Suffix, None),
    ("acustica", Kind::SessionWeak, Scope::Suffix, None),
    ("acoustique", Kind::SessionWeak, Scope::Suffix, None),
    ("akustik", Kind::SessionWeak, Scope::Suffix, None),
    ("akustisch", Kind::SessionWeak, Scope::Suffix, None),
    ("stripped", Kind::SessionWeak, Scope::Suffix, None),
    ("piano version", Kind::SessionWeak, Scope::Suffix, None),
    // Remixes, edits and alternate versions.
    ("remix", Kind::Remix, Scope::Anywhere, None),
    ("remixed", Kind::Remix, Scope::Anywhere, None),
    ("remixes", Kind::Remix, Scope::Anywhere, None),
    ("rmx", Kind::Remix, Scope::Anywhere, None),
    ("rework", Kind::Remix, Scope::Anywhere, None),
    ("edit", Kind::Remix, Scope::Suffix, None),
    ("version", Kind::Remix, Scope::Suffix, None),
    ("versions", Kind::Remix, Scope::Suffix, None),
    ("versao", Kind::Remix, Scope::Suffix, None),
    ("versione", Kind::Remix, Scope::Suffix, None),
    ("mix", Kind::Remix, Scope::Suffix, Some("original")),
    ("extended", Kind::Remix, Scope::Suffix, None),
    ("instrumental", Kind::Remix, Scope::Suffix, None),
    ("dub", Kind::Remix, Scope::Suffix, None),
    ("sped up", Kind::Remix, Scope::Suffix, None),
    ("slowed", Kind::Remix, Scope::Suffix, None),
    ("reprise", Kind::Remix, Scope::Suffix, None),
    // Explicit official-video markers (only used to veto weak interviews).
    ("official video", Kind::Official, Scope::Anywhere, None),
    (
        "official music video",
        Kind::Official,
        Scope::Anywhere,
        None,
    ),
    ("official hd video", Kind::Official, Scope::Anywhere, None),
    ("official mv", Kind::Official, Scope::Anywhere, None),
    ("music video", Kind::Official, Scope::Anywhere, None),
    ("video oficial", Kind::Official, Scope::Anywhere, None),
    ("videoclip", Kind::Official, Scope::Anywhere, None),
    ("videoclipe", Kind::Official, Scope::Anywhere, None),
    ("clipe oficial", Kind::Official, Scope::Anywhere, None),
    ("clip oficial", Kind::Official, Scope::Anywhere, None),
    ("clip officiel", Kind::Official, Scope::Anywhere, None),
    ("video officiel", Kind::Official, Scope::Anywhere, None),
    ("video ufficiale", Kind::Official, Scope::Anywhere, None),
    (
        "offizielles musikvideo",
        Kind::Official,
        Scope::Anywhere,
        None,
    ),
    ("offizielles video", Kind::Official, Scope::Anywhere, None),
];

/// Markers indexed by their first word, so a title is scanned once.
static INDEX: LazyLock<HashMap<&'static str, Vec<Marker>>> = LazyLock::new(|| {
    let mut index: HashMap<&'static str, Vec<Marker>> = HashMap::new();
    for (phrase, kind, scope, unless_after) in MARKERS {
        let words: Vec<&'static str> = phrase.split(' ').collect();
        index.entry(words[0]).or_default().push(Marker {
            words,
            kind: *kind,
            scope: *scope,
            unless_after: *unless_after,
        });
    }
    index
});

/// Lowercase, strip accents, punctuation to spaces.
pub fn fold(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    // Most titles are ASCII: skip Unicode decomposition for them.
    if text.is_ascii() {
        for byte in text.bytes() {
            folded.push(if byte.is_ascii_alphanumeric() {
                char::from(byte.to_ascii_lowercase())
            } else {
                ' '
            });
        }
        return folded;
    }
    for c in text
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
    {
        if c.is_alphanumeric() {
            folded.push(c);
        } else {
            folded.push(' ');
        }
    }
    folded
}

/// " - ", en dash, em dash, " | " or " // " at `pos`; returns its byte length.
fn separator_at(title: &str, pos: usize) -> Option<usize> {
    const SEPARATORS: [&str; 5] = [" - ", " \u{2013} ", " \u{2014} ", " | ", " // "];
    let rest = &title[pos..];
    SEPARATORS
        .iter()
        .find(|sep| rest.starts_with(**sep))
        .map(|sep| sep.len())
}

fn opener(c: char) -> bool {
    matches!(c, '(' | '[' | '{')
}

fn closer(c: char) -> bool {
    matches!(c, ')' | ']' | '}')
}

/// A title split into its base (the song) and its suffix segments: each
/// bracket group and each dash part after the base, in order, trimmed.
/// A leading bracket group ("(I Can't Get No) Satisfaction") stays in the
/// base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleParts {
    pub base: String,
    /// Byte offset in the title where the suffix starts.
    pub suffix_at: usize,
    pub segments: Vec<String>,
}

/// Byte offset where a title's suffix starts: the first bracket or dash
/// separator after some base text. The title's length when there is none.
pub fn suffix_start(title: &str) -> usize {
    let mut depth = 0usize;
    let mut seen_text = false;
    for (pos, c) in title.char_indices() {
        if depth > 0 {
            if opener(c) {
                depth += 1;
            } else if closer(c) {
                depth -= 1;
            }
            seen_text |= !c.is_whitespace();
            continue;
        }
        if opener(c) {
            if seen_text {
                return pos;
            }
            // A leading group belongs to the song's name.
            depth = 1;
            continue;
        }
        if seen_text && separator_at(title, pos).is_some() {
            return pos;
        }
        seen_text |= !c.is_whitespace();
    }
    title.len()
}

pub fn title_parts(title: &str) -> TitleParts {
    let suffix_at = suffix_start(title);
    let suffix = &title[suffix_at..];
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut skip_until = 0usize;
    let flush = |current: &mut String, segments: &mut Vec<String>| {
        let trimmed = current
            .trim()
            .trim_matches(|c: char| c == ',' || c == ':')
            .trim();
        if !trimmed.is_empty() {
            segments.push(trimmed.to_string());
        }
        current.clear();
    };
    for (pos, c) in suffix.char_indices() {
        if pos < skip_until {
            continue;
        }
        if depth > 0 {
            if opener(c) {
                depth += 1;
            } else if closer(c) {
                depth -= 1;
                if depth == 0 {
                    flush(&mut current, &mut segments);
                    continue;
                }
            }
            current.push(c);
            continue;
        }
        if opener(c) {
            flush(&mut current, &mut segments);
            depth = 1;
            continue;
        }
        if closer(c) {
            continue;
        }
        if let Some(len) = separator_at(suffix, pos) {
            flush(&mut current, &mut segments);
            skip_until = pos + len;
            continue;
        }
        current.push(c);
    }
    flush(&mut current, &mut segments);
    TitleParts {
        base: title[..suffix_at].trim().to_string(),
        suffix_at,
        segments,
    }
}

/// Which marker kinds a title carries.
fn markers(title: &str) -> u16 {
    let suffix_at = suffix_start(title);
    marker_bits(&title[..suffix_at], &title[suffix_at..])
}

/// Whether a suffix segment ("Live at Wembley", "Ao Vivo Em Sao Paulo")
/// says live, reading every word as suffix.
pub fn segment_is_live(segment: &str) -> bool {
    has(marker_bits("", segment), Kind::Live)
}

/// Whether a suffix segment names a session format or a plain session word.
pub fn segment_is_session(segment: &str) -> bool {
    let bits = marker_bits("", segment);
    has(bits, Kind::SessionStrong) || has(bits, Kind::SessionWeak)
}

fn marker_bits(base: &str, suffix: &str) -> u16 {
    let base = fold(base);
    let suffix = fold(suffix);
    let base_words: Vec<&str> = base.split_whitespace().collect();
    let words: Vec<&str> = base_words
        .iter()
        .copied()
        .chain(suffix.split_whitespace())
        .collect();
    let suffix_from = base_words.len();
    let mut bits = 0u16;
    for (i, word) in words.iter().enumerate() {
        let Some(candidates) = INDEX.get(*word) else {
            continue;
        };
        for marker in candidates {
            if marker.scope == Scope::Suffix && i < suffix_from {
                continue;
            }
            if words.len() < i + marker.words.len()
                || words[i..i + marker.words.len()] != marker.words[..]
            {
                continue;
            }
            if let Some(blocker) = marker.unless_after
                && i > 0
                && words[i - 1] == blocker
            {
                continue;
            }
            bits |= marker.kind.bit();
        }
    }
    // A base that ends in "Trailer" or "Interview" names what it is
    // ("Album Trailer", "Live At River Plate Trailer"); a song that starts
    // with the word ("Trailer Park") does not.
    if base_words.len() > 1 {
        match base_words.last().copied() {
            Some("trailer" | "teaser") => bits |= Kind::Trailer.bit(),
            Some("interview" | "commentary") => bits |= Kind::Interview.bit(),
            _ => {}
        }
    } else if matches!(base_words.first().copied(), Some("interview")) {
        bits |= Kind::Interview.bit();
    }
    bits
}

fn has(bits: u16, kind: Kind) -> bool {
    bits & kind.bit() != 0
}

/// Classify one video. Order: duration under 90 s is a clip; then
/// interviews and trailers (title or TIDAL type); 30 min and up is long form
/// unless it is a concert or a session; then dance, lyric, visualizer,
/// session formats, live, plain session words, remix; TIDAL's "Live" type
/// breaks the tie; everything else is the official video.
pub fn classify(title: &str, video_type: Option<&str>, duration_s: Option<i64>) -> Cut {
    if duration_s.is_some_and(|d| d < CLIP_UNDER_S) {
        return Cut::Clip;
    }
    let bits = markers(title);
    let kind = video_type.unwrap_or("");
    if matches!(kind, "Interview" | "Documentary")
        || has(bits, Kind::Interview)
        || (has(bits, Kind::WeakInterview) && !has(bits, Kind::Official))
    {
        return Cut::Interview;
    }
    if kind == "Promotional" || has(bits, Kind::Trailer) {
        return Cut::Trailer;
    }
    if duration_s.is_some_and(|d| d >= LONGFORM_FROM_S) {
        return if has(bits, Kind::SessionStrong) {
            Cut::Session
        } else if has(bits, Kind::Live) {
            Cut::Live
        } else if has(bits, Kind::SessionWeak) {
            Cut::Session
        } else {
            Cut::Longform
        };
    }
    let ordered = [
        (Kind::Dance, Cut::Dance),
        (Kind::Lyric, Cut::Lyric),
        (Kind::Visualizer, Cut::Visualizer),
        (Kind::SessionStrong, Cut::Session),
        (Kind::Live, Cut::Live),
        (Kind::SessionWeak, Cut::Session),
        (Kind::Remix, Cut::Remix),
    ];
    if let Some((_, cut)) = ordered.iter().find(|(kind, _)| has(bits, *kind)) {
        return *cut;
    }
    if matches!(kind, "Live" | "Event") {
        return Cut::Live;
    }
    Cut::Official
}

/// Words a suffix may hold while the video is still "the" video of its song:
/// official markers, remaster notes, resolution.
const PLAIN_WORDS: &[&str] = &[
    "official",
    "video",
    "music",
    "musical",
    "mv",
    "m",
    "v",
    "hd",
    "4k",
    "uhd",
    "remastered",
    "remaster",
    "oficial",
    "videoclip",
    "videoclipe",
    "clip",
    "clipe",
    "officiel",
    "ufficiale",
    "offizielles",
    "musikvideo",
    "explicit",
    "clean",
    "dirty",
    "uncensored",
];

/// Whether a title is the song's plain video: no suffix, or only official
/// markers, remaster notes, years and featured-artist credits. Among cuts of
/// one kind the plain one fronts the song ("Last Christmas" over "Last
/// Christmas (Vevo Footnotes)").
pub fn is_plain_title(title: &str) -> bool {
    title_parts(title).segments.iter().all(|segment| {
        let folded = fold(segment);
        let mut words = folded.split_whitespace().peekable();
        if matches!(
            words.peek().copied(),
            Some("feat" | "ft" | "featuring" | "with")
        ) {
            return true;
        }
        words.all(|word| PLAIN_WORDS.contains(&word) || word.chars().all(|c| c.is_ascii_digit()))
    })
}

/// Compilations under "Various Artists" are never programmable.
pub fn is_various_artists(artist_id: Option<i64>, artist_name: Option<&str>) -> bool {
    artist_id == Some(VARIOUS_ARTISTS_ID)
        || artist_name.is_some_and(|name| name_key(name) == "various artists")
}

/// The one programmable rule: not a clip, interview or trailer, and not a
/// Various Artists compilation.
pub fn programmable(cut: Cut, artist_id: Option<i64>, artist_name: Option<&str>) -> bool {
    cut.is_programmable() && !is_various_artists(artist_id, artist_name)
}

pub fn candidate_cut(video: &VideoCandidate) -> Cut {
    classify(&video.title, video.video_type.as_deref(), video.duration_s)
}

/// Whether any station, radio or shelf may play this video.
pub fn is_programmable(video: &VideoCandidate) -> bool {
    programmable(
        candidate_cut(video),
        video.artist_id,
        video.artist_name.as_deref(),
    )
}

/// Whether a title says it was recorded live (any live marker, wherever it
/// sits), for concert labels.
pub fn has_live_marker(title: &str) -> bool {
    has(markers(title), Kind::Live)
}

/// quality = popularity percentile ^ 0.5 x runtime sanity x artwork present.
/// An unknown popularity counts as the median.
pub fn quality(popularity_percentile: Option<f64>, duration_s: Option<i64>, artwork: bool) -> f64 {
    let popularity = popularity_percentile.unwrap_or(0.5).clamp(0.0, 1.0).sqrt();
    let runtime = match duration_s {
        None => 0.9,
        Some(d) if d < CLIP_UNDER_S => 0.3,
        Some(d) if d < 150 => 0.85,
        Some(d) if d <= 480 => 1.0,
        Some(d) if d <= 900 => 0.9,
        Some(_) => 0.8,
    };
    let art = if artwork { 1.0 } else { 0.6 };
    popularity * runtime * art
}

/// Mid-rank percentile of each popularity value among all known values.
fn percentiles(values: &[i32]) -> HashMap<i32, f64> {
    let mut counts: BTreeMap<i32, usize> = BTreeMap::new();
    for value in values {
        *counts.entry(*value).or_default() += 1;
    }
    let total = values.len().max(1) as f64;
    let mut below = 0usize;
    let mut out = HashMap::with_capacity(counts.len());
    for (value, count) in counts {
        out.insert(value, (below as f64 + count as f64 / 2.0) / total);
        below += count;
    }
    out
}

static MAX_YEAR: LazyLock<i64> =
    LazyLock::new(|| i64::from(chrono::Datelike::year(&chrono::Local::now())) + 1);

/// A plausible four-digit year, else None (TIDAL sends 1900 placeholders).
pub fn plausible_year(year: Option<i64>) -> Option<i64> {
    year.filter(|y| (1920..=*MAX_YEAR).contains(y))
}

struct FacetRow {
    id: i64,
    artist_id: Option<i64>,
    cut: Cut,
    programmable: bool,
    song_key: String,
    cut_year: Option<i64>,
    duration_s: Option<i64>,
    popularity: Option<i32>,
    artwork: bool,
}

/// Rebuild video_facets from video_catalog. Concert ids survive the rebuild
/// (the concerts stage rewrites them right after) so readers never see a
/// gap. Returns the number of rows written.
pub fn build(conn: &Connection) -> Result<usize> {
    let previous_concerts: HashMap<i64, i64> = {
        let mut stmt = conn.prepare(
            "SELECT tidal_video_id, concert_id FROM video_facets WHERE concert_id IS NOT NULL",
        )?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?
    };
    let mut rows = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT tidal_video_id, artist_tidal_id, artist_name, item_json FROM video_catalog",
        )?;
        let mut query = stmt.query([])?;
        while let Some(row) = query.next()? {
            let id: i64 = row.get(0)?;
            let column_artist: Option<i64> = row.get(1)?;
            let column_name: Option<String> = row.get(2)?;
            let json: String = row.get(3)?;
            let Ok(video) = serde_json::from_str::<VideoCandidate>(&json) else {
                continue;
            };
            let artist_id = column_artist.filter(|a| *a > 0).or(video.artist_id);
            let artist_name = column_name
                .filter(|name| !name.is_empty())
                .or(video.artist_name.clone());
            let cut = candidate_cut(&video);
            rows.push(FacetRow {
                id,
                artist_id,
                cut,
                programmable: programmable(cut, artist_id, artist_name.as_deref()),
                song_key: video_song_key(artist_id, artist_name.as_deref(), &video.title),
                cut_year: plausible_year(video.release_year.map(i64::from)),
                duration_s: video.duration_s,
                popularity: video.popularity,
                artwork: video.artwork_url.as_deref().is_some_and(|u| !u.is_empty()),
            });
        }
    }
    let known: Vec<i32> = rows.iter().filter_map(|row| row.popularity).collect();
    let percentile = percentiles(&known);
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM video_facets", [])?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO video_facets (tidal_video_id, artist_tidal_id, cut, programmable,
                 song_key, concert_id, cut_year, duration_s, popularity, quality)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        for row in &rows {
            let quality = quality(
                row.popularity.and_then(|p| percentile.get(&p).copied()),
                row.duration_s,
                row.artwork,
            );
            insert.execute(params![
                row.id,
                row.artist_id,
                row.cut.as_str(),
                row.programmable,
                row.song_key,
                previous_concerts.get(&row.id),
                row.cut_year,
                row.duration_s,
                row.popularity,
                quality,
            ])?;
        }
    }
    tx.commit()?;
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cut(title: &str) -> Cut {
        classify(title, Some("Music Video"), Some(230))
    }

    #[test]
    fn english_markers() {
        assert_eq!(
            cut("Hit You With The Real Thing (Live At Croke Park Stadium)"),
            Cut::Live
        );
        assert_eq!(cut("Bohemian Rhapsody - Live Aid"), Cut::Live);
        assert_eq!(
            cut("Sweet Jane (Official Remastered HD Video)"),
            Cut::Official
        );
        assert_eq!(
            cut("Stay (If You Wanna Dance) (Official Lyric Video)"),
            Cut::Lyric
        );
        assert_eq!(cut("Blinding Lights (Official Audio)"), Cut::Visualizer);
        assert_eq!(cut("Ready - A COLORS SHOW"), Cut::Session);
        assert_eq!(cut("Waking Up (Eclipse Live Session)"), Cut::Session);
        assert_eq!(cut("Missed Connection (Stripped)"), Cut::Session);
        assert_eq!(cut("Let It Go (Baauer Remix)"), Cut::Remix);
        assert_eq!(cut("Sister Dew (Radio Edit - Stereo)"), Cut::Remix);
        assert_eq!(cut("Me On You (Behind The Scenes)"), Cut::Interview);
        assert_eq!(cut("Gypsy - The Making of the Video"), Cut::Interview);
        assert_eq!(cut("Why Do I Feel so Sad (Commentary)"), Cut::Interview);
        assert_eq!(cut("Dark Night of the Soul (Album Trailer)"), Cut::Trailer);
        assert_eq!(cut("Album Teaser"), Cut::Trailer);
        assert_eq!(cut("Shine On Us (Choreography Video)"), Cut::Dance);
        assert_eq!(cut("the 1 (Vertical Video)"), Cut::Dance);
    }

    #[test]
    fn spanish_portuguese_french_italian_german_markers() {
        assert_eq!(cut("Tormento de Amor (En Vivo)"), Cut::Live);
        assert_eq!(cut("Evidencias - Ao Vivo"), Cut::Live);
        assert_eq!(cut("\u{c9} Bom Demais / Outdoor (Ao Vivo)"), Cut::Live);
        assert_eq!(cut("Sonho de Amor (Ao Vivo Em S\u{e3}o Paulo)"), Cut::Live);
        assert_eq!(cut("Corazon Partio (En Directo)"), Cut::Live);
        assert_eq!(cut("Vuelvo A Traficar (Live)"), Cut::Live);
        assert_eq!(
            cut("Acu\u{e9}rdate de M\u{ed} (Ac\u{fa}stico)"),
            Cut::Session
        );
        assert_eq!(
            cut("Para M\u{ed} (Versi\u{f3}n Ac\u{fa}stica)"),
            Cut::Session
        );
        assert_eq!(cut("Despacito (Letra)"), Cut::Lyric);
        assert_eq!(cut("Ma direction (Paroles)"), Cut::Lyric);
        assert_eq!(cut("Ciao (Testo)"), Cut::Lyric);
        assert_eq!(cut("Bailando (Video Oficial)"), Cut::Official);
        assert_eq!(cut("Derni\u{e8}re danse (Clip officiel)"), Cut::Official);
        assert_eq!(cut("Carlos Rivera - Yo Creo (Documental)"), Cut::Interview);
        assert_eq!(
            cut("Je chanterai - Derri\u{e8}re les coulisses"),
            Cut::Interview
        );
        assert_eq!(
            cut("El Nene la amenaza - Entrevista en 0 es 3"),
            Cut::Interview
        );
        assert_eq!(cut("Herencia (Entrevista parte I)"), Cut::Interview);
        assert_eq!(cut("Hermanos (Vers\u{e3}o em Portugu\u{ea}s)"), Cut::Remix);
        assert_eq!(cut("Sembro matto (Dal Vivo)"), Cut::Live);
        assert_eq!(cut("4 Uhr 30 (Offizielles Making-Of)"), Cut::Interview);
        assert_eq!(cut("Atemlos (Live aus Berlin)"), Cut::Live);
        assert_eq!(cut("Lass mich (Akustik Version)"), Cut::Session);
    }

    #[test]
    fn song_titles_that_contain_marker_words_stay_official() {
        for title in [
            "Live Forever",
            "Live While We're Young",
            "Live and Learn",
            "Trailer Park",
            "Jesus Is Coming Soon",
            "Chain Reaction",
            "Years In The Making",
            "Paroles, paroles",
            "True Colors (Official Video)",
            "Concert For George",
            "The Grand Tour",
            "Left and Right (Feat. Jung Kook of BTS)",
            "Psychosocial (2022 Remaster)",
            "La Mision (Episodio 2) [Official Video]",
            "Soundscape - Original Mix",
        ] {
            assert_eq!(cut(title), Cut::Official, "{title}");
        }
    }

    #[test]
    fn duration_and_type_rules() {
        assert_eq!(classify("Song (Live)", None, Some(45)), Cut::Clip);
        assert_eq!(classify("Song", None, Some(89)), Cut::Clip);
        assert_eq!(classify("Song", None, Some(90)), Cut::Official);
        assert_eq!(classify("Full Show", None, Some(4000)), Cut::Longform);
        assert_eq!(
            classify("Live at Pompeii (Live)", None, Some(4000)),
            Cut::Live
        );
        assert_eq!(
            classify("Tiny Desk (Home) Concert", None, Some(1990)),
            Cut::Session
        );
        assert_eq!(
            classify("Big Freedia", Some("Interview"), Some(213)),
            Cut::Interview
        );
        assert_eq!(
            classify("J Balvin", Some("Documentary"), Some(1190)),
            Cut::Interview
        );
        assert_eq!(
            classify("Surrender", Some("Promotional"), Some(101)),
            Cut::Trailer
        );
        assert_eq!(classify("Liberation", Some("Live"), Some(189)), Cut::Live);
        assert_eq!(classify("Song", None, None), Cut::Official);
    }

    #[test]
    fn bts_is_behind_the_scenes_unless_it_is_the_band() {
        assert_eq!(cut("Drive (feat. Wes Nelson) [BTS]"), Cut::Interview);
        assert_eq!(cut("Blueberry Eyes (feat. SUGA of BTS)"), Cut::Official);
    }

    #[test]
    fn programmable_rule() {
        assert!(programmable(Cut::Official, Some(1), Some("Lorde")));
        assert!(programmable(Cut::Visualizer, Some(1), Some("Lorde")));
        assert!(programmable(Cut::Longform, Some(1), Some("Lorde")));
        assert!(!programmable(Cut::Clip, Some(1), Some("Lorde")));
        assert!(!programmable(Cut::Interview, Some(1), Some("Lorde")));
        assert!(!programmable(Cut::Trailer, Some(1), Some("Lorde")));
        assert!(!programmable(Cut::Official, Some(VARIOUS_ARTISTS_ID), None));
        assert!(!programmable(
            Cut::Official,
            Some(7),
            Some("Various Artists")
        ));
    }

    #[test]
    fn title_parts_split_base_and_segments() {
        let parts = title_parts(
            "Black Hearted Woman (Live at Beacon Theatre, New York, NY, 3/26/2009) [Remastered 2021]",
        );
        assert_eq!(parts.base, "Black Hearted Woman");
        assert_eq!(
            parts.segments,
            vec![
                "Live at Beacon Theatre, New York, NY, 3/26/2009".to_string(),
                "Remastered 2021".to_string()
            ]
        );
        let parts = title_parts("(I Can't Get No) Satisfaction - Live");
        assert_eq!(parts.base, "(I Can't Get No) Satisfaction");
        assert_eq!(parts.segments, vec!["Live".to_string()]);
        let parts = title_parts("El Hubiera No Existe (Documental - Capitulo 3 (Video))");
        assert_eq!(
            parts.segments,
            vec!["Documental - Capitulo 3 (Video)".to_string()]
        );
        assert_eq!(title_parts("Plain").segments, Vec::<String>::new());
    }

    #[test]
    fn plain_titles_are_the_song_itself() {
        assert!(is_plain_title("Last Christmas"));
        assert!(is_plain_title("Last Christmas (Official 4K Video)"));
        assert!(is_plain_title("Juicy (feat. Total) [2009 Remaster]"));
        assert!(!is_plain_title("Last Christmas (Vevo Footnotes)"));
        assert!(!is_plain_title(
            "Last Christmas (Live at Wembley Arena, December 2006)"
        ));
        assert_eq!(
            cut("Last Christmas (35th Anniversary Story Behind the Video)"),
            Cut::Interview
        );
    }

    #[test]
    fn quality_rewards_popularity_runtime_and_artwork() {
        let hit = quality(Some(1.0), Some(240), true);
        let unknown = quality(None, Some(240), true);
        let short = quality(Some(1.0), Some(60), true);
        let no_art = quality(Some(1.0), Some(240), false);
        assert!((hit - 1.0).abs() < 1e-9);
        assert!(unknown < hit && unknown > 0.6);
        assert!(short < hit && no_art < hit);
    }

    #[test]
    fn percentiles_use_mid_ranks() {
        let p = percentiles(&[0, 0, 10, 20]);
        assert!((p[&0] - 0.25).abs() < 1e-9);
        assert!((p[&10] - 0.625).abs() < 1e-9);
        assert!((p[&20] - 0.875).abs() < 1e-9);
    }
}
