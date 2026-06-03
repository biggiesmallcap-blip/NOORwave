use crate::genre::mappings::{GenreCatalog, GenreResolution};
use std::collections::BTreeSet;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct GenreAssignmentBuilder {
    catalog: GenreCatalog,
}

impl GenreAssignmentBuilder {
    pub fn from_embedded() -> Self {
        Self {
            catalog: GenreCatalog::from_embedded(),
        }
    }

    pub fn catalog(&self) -> &GenreCatalog {
        &self.catalog
    }

    pub fn resolve(&self, raw: &str) -> GenreResolution {
        self.catalog.resolve(raw)
    }

    pub fn normalize(&self, raw: &str) -> Option<String> {
        let resolution = self.resolve(raw);
        resolution.canonical_name().map(str::to_string)
    }
}

pub fn embedded_builder() -> &'static GenreAssignmentBuilder {
    static BUILDER: OnceLock<GenreAssignmentBuilder> = OnceLock::new();
    BUILDER.get_or_init(GenreAssignmentBuilder::from_embedded)
}

pub fn collect_clear_genres<I, S>(values: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut genres = BTreeSet::new();

    for value in values {
        let resolution = embedded_builder().resolve(value.as_ref());
        if let Some(canonical) = resolution.canonical_name() {
            genres.insert(canonical.to_string());
        }
    }

    genres.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{GenreAssignmentBuilder, collect_clear_genres};

    #[test]
    fn resolves_single_genres() {
        let builder = GenreAssignmentBuilder::from_embedded();
        assert_eq!(builder.normalize("trip hop").as_deref(), Some("Trip-Hop"));
        assert_eq!(builder.normalize("shoegazee").as_deref(), Some("Shoegaze"));
    }

    #[test]
    fn fails_closed_on_ambiguous_compound_inputs() {
        let builder = GenreAssignmentBuilder::from_embedded();
        assert_eq!(builder.normalize("Tech House / House"), None);
    }

    #[test]
    fn collects_only_clear_canonical_genres() {
        assert_eq!(
            collect_clear_genres(["trip hop", "shoegazee", "Tech House / House", ""]),
            vec!["Shoegaze".to_string(), "Trip-Hop".to_string()]
        );
    }
}
