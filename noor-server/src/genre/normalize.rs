#![allow(dead_code)]

use crate::genre::builder::embedded_builder;

pub struct GenreNormalizer;

impl GenreNormalizer {
    pub fn from_embedded() -> Self {
        Self
    }

    pub fn normalize(&self, raw: &str) -> Option<String> {
        embedded_builder().normalize(raw)
    }
}

pub fn normalize_genre_name(raw: &str) -> Option<String> {
    embedded_builder().normalize(raw)
}

#[cfg(test)]
mod tests {
    use super::{GenreNormalizer, normalize_genre_name};

    #[test]
    fn resolves_alias_matches() {
        let normalizer = GenreNormalizer::from_embedded();
        assert_eq!(
            normalizer.normalize("trip hop").as_deref(),
            Some("Trip-Hop")
        );
        assert_eq!(
            normalizer.normalize("dnb").as_deref(),
            Some("Drum and Bass")
        );
    }

    #[test]
    fn resolves_canonical_matches() {
        let normalizer = GenreNormalizer::from_embedded();
        assert_eq!(normalizer.normalize("House").as_deref(), Some("House"));
        assert_eq!(
            normalizer.normalize("post rock").as_deref(),
            Some("Post-Rock")
        );
    }

    #[test]
    fn resolves_fuzzy_matches() {
        let normalizer = GenreNormalizer::from_embedded();
        assert_eq!(
            normalizer.normalize("shoegazee").as_deref(),
            Some("Shoegaze")
        );
        assert_eq!(
            normalizer.normalize("progessive house").as_deref(),
            Some("Progressive House")
        );
    }

    #[test]
    fn fails_closed_on_ambiguous_compound_inputs() {
        assert_eq!(normalize_genre_name("Tech House / House"), None);
        assert_eq!(normalize_genre_name("   "), None);
    }

    #[test]
    fn ignores_taxonomy_root_label() {
        assert_eq!(normalize_genre_name("Genres"), None);
    }
}
