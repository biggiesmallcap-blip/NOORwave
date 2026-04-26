use rusqlite::Connection;

// Common non-genre tags that should never reach genre resolution.
const STOP_TAGS: &[&str] = &[
    "seen live",
    "seen-live",
    "favourite",
    "favourites",
    "favorite",
    "favorites",
    "love",
    "loved",
    "owned",
    "albums i own",
    "albums-i-own",
    "to listen to",
    "to-listen-to",
    "to check out",
    "rip",
    "amazing",
    "awesome",
    "cool",
    "good",
    "great",
    "best",
    "classic",
    "classics",
    "all time favorites",
    "all-time",
    "epic",
    "legendary",
    "underrated",
    "overrated",
    "male vocalists",
    "female vocalists",
    "male vocalist",
    "female vocalist",
    "vocalists",
    "vocalist",
    "instrumental",
    "singer-songwriter",
    "singer songwriter",
    "albums",
    "artist",
    "artists",
    "band",
    "bands",
    "playlist",
    "mp3",
    "download",
    "streaming",
];

// Geography terms that appear frequently as Last.fm tags but are not genres.
const LOCALES: &[&str] = &[
    "american",
    "australian",
    "austrian",
    "belgian",
    "brazilian",
    "british",
    "canadian",
    "chinese",
    "czech",
    "danish",
    "dutch",
    "english",
    "finnish",
    "french",
    "german",
    "greek",
    "hungarian",
    "indian",
    "iranian",
    "irish",
    "italian",
    "japanese",
    "korean",
    "mexican",
    "norwegian",
    "polish",
    "portuguese",
    "romanian",
    "russian",
    "scandinavian",
    "scottish",
    "spanish",
    "swedish",
    "swiss",
    "turkish",
    "ukrainian",
    // Cities / regions
    "london",
    "new york",
    "chicago",
    "detroit",
    "berlin",
    "paris",
    "tokyo",
    "los angeles",
    "la",
    "nyc",
    "uk",
    "usa",
    "us",
    "europe",
    "european",
    "nordic",
    "latin american",
    "south american",
];

/// Returns `true` if this tag is worth sending to genre resolution.
///
/// Rejects: stop tags, locales, decade markers (e.g. "90s"), excessively long
/// strings, and any string that matches an artist name already in the DB.
pub fn should_keep_tag(tag: &str, conn: &Connection) -> bool {
    let trimmed = tag.trim();

    if trimmed.is_empty() || trimmed.len() > 40 {
        return false;
    }

    // Decade / year markers: "90s", "2010s", "70's", etc.
    // Strip optional trailing 's or s, then check if only digits remain.
    let stripped = trimmed.trim_end_matches("'s").trim_end_matches('s');
    if !stripped.is_empty() && stripped.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    let lower = trimmed.to_ascii_lowercase();

    if STOP_TAGS.iter().any(|&t| t == lower) {
        return false;
    }

    if LOCALES.iter().any(|&l| l == lower) {
        return false;
    }

    // Reject if this is an artist name in the local library.
    let is_artist = conn
        .query_row(
            "SELECT COUNT(*) FROM artists WHERE lower(name) = ?1",
            [&lower],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    !is_artist
}
