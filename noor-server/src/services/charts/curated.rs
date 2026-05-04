//! Curated genre and country lists for Last.fm chart endpoints.
//!
//! These power `/api/charts/lastfm/genres` and `/api/charts/lastfm/countries`
//! and are also consumed by `fetch_lastfm_chart` to map canonical keys/codes
//! to the strings Last.fm expects (genre tag(s), full country name).

pub struct GenreEntry {
    pub key: &'static str,
    pub label: &'static str,
    pub lastfm_tags: &'static [&'static str],
}

pub struct CountryEntry {
    pub code: &'static str,
    pub lastfm_name: &'static str,
    pub label: &'static str,
}

pub const CURATED_GENRES: &[GenreEntry] = &[
    GenreEntry {
        key: "electronic",
        label: "Electronic",
        lastfm_tags: &["electronic"],
    },
    GenreEntry {
        key: "rock",
        label: "Rock",
        lastfm_tags: &["rock"],
    },
    GenreEntry {
        key: "pop",
        label: "Pop",
        lastfm_tags: &["pop"],
    },
    GenreEntry {
        key: "hip-hop",
        label: "Hip-Hop",
        lastfm_tags: &["hip-hop", "hip hop"],
    },
    GenreEntry {
        key: "rnb",
        label: "R&B",
        lastfm_tags: &["rnb", "r&b"],
    },
    GenreEntry {
        key: "jazz",
        label: "Jazz",
        lastfm_tags: &["jazz"],
    },
    GenreEntry {
        key: "metal",
        label: "Metal",
        lastfm_tags: &["metal"],
    },
    GenreEntry {
        key: "folk",
        label: "Folk",
        lastfm_tags: &["folk"],
    },
    GenreEntry {
        key: "soul",
        label: "Soul",
        lastfm_tags: &["soul"],
    },
    GenreEntry {
        key: "punk",
        label: "Punk",
        lastfm_tags: &["punk"],
    },
    GenreEntry {
        key: "indie",
        label: "Indie",
        lastfm_tags: &["indie"],
    },
    GenreEntry {
        key: "classical",
        label: "Classical",
        lastfm_tags: &["classical"],
    },
    GenreEntry {
        key: "drum-and-bass",
        label: "Drum and Bass",
        lastfm_tags: &["drum and bass", "dnb"],
    },
];

// AU first — user is in AU; this also drives the default chip on Home/Search.
pub const CURATED_COUNTRIES: &[CountryEntry] = &[
    CountryEntry {
        code: "AU",
        lastfm_name: "Australia",
        label: "Australia",
    },
    CountryEntry {
        code: "US",
        lastfm_name: "United States",
        label: "United States",
    },
    CountryEntry {
        code: "GB",
        lastfm_name: "United Kingdom",
        label: "United Kingdom",
    },
    CountryEntry {
        code: "JP",
        lastfm_name: "Japan",
        label: "Japan",
    },
    CountryEntry {
        code: "BR",
        lastfm_name: "Brazil",
        label: "Brazil",
    },
    CountryEntry {
        code: "DE",
        lastfm_name: "Germany",
        label: "Germany",
    },
    CountryEntry {
        code: "FR",
        lastfm_name: "France",
        label: "France",
    },
    CountryEntry {
        code: "CA",
        lastfm_name: "Canada",
        label: "Canada",
    },
];

pub fn find_genre(key: &str) -> Option<&'static GenreEntry> {
    let needle = key.trim().to_ascii_lowercase();
    CURATED_GENRES
        .iter()
        .find(|g| g.key.eq_ignore_ascii_case(&needle))
}

pub fn find_country_by_code(code: &str) -> Option<&'static CountryEntry> {
    let needle = code.trim().to_ascii_uppercase();
    CURATED_COUNTRIES.iter().find(|c| c.code == needle.as_str())
}

/// Match a curated country by ISO code OR by Last.fm full name (case-insensitive).
/// Used to canonicalise cache keys so `?country=AU` and `?country=Australia`
/// hit the same cache entry.
pub fn find_country_by_code_or_name(input: &str) -> Option<&'static CountryEntry> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() == 2 {
        return find_country_by_code(trimmed);
    }
    CURATED_COUNTRIES
        .iter()
        .find(|c| c.lastfm_name.eq_ignore_ascii_case(trimmed))
}

pub const DEFAULT_COUNTRY_CODE: &str = "AU";
pub const DEFAULT_GENRE_KEY: &str = "electronic";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_resolve() {
        assert!(find_country_by_code(DEFAULT_COUNTRY_CODE).is_some());
        assert!(find_genre(DEFAULT_GENRE_KEY).is_some());
    }

    #[test]
    fn country_lookup_is_case_insensitive() {
        assert_eq!(find_country_by_code("au").map(|c| c.code), Some("AU"));
        assert_eq!(find_country_by_code("AU").map(|c| c.code), Some("AU"));
        assert!(find_country_by_code("ZZ").is_none());
    }

    #[test]
    fn country_or_name_canonicalises_to_iso() {
        assert_eq!(
            find_country_by_code_or_name("AU").map(|c| c.code),
            Some("AU")
        );
        assert_eq!(
            find_country_by_code_or_name("Australia").map(|c| c.code),
            Some("AU")
        );
        assert_eq!(
            find_country_by_code_or_name("united states").map(|c| c.code),
            Some("US")
        );
        assert!(find_country_by_code_or_name("Atlantis").is_none());
    }

    #[test]
    fn genre_lookup_finds_aliased_keys() {
        let hh = find_genre("hip-hop").unwrap();
        assert_eq!(hh.lastfm_tags.len(), 2);
        assert!(find_genre("Hip-Hop").is_some());
        assert!(find_genre("rnb").is_some());
        assert!(find_genre("nonexistent").is_none());
    }
}
