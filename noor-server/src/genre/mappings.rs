use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const TAXONOMY_JSON: &str = include_str!("../../../genre-taxonomy/taxonomy.json");
const ALIASES_JSON: &str = include_str!("../../../genre-taxonomy/aliases.json");

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TaxonomyNode {
    pub name: String,
    #[serde(default)]
    pub children: Vec<TaxonomyNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreEntry {
    pub name: String,
    pub slug: String,
    pub paths: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MatchKind {
    ExactCanonical,
    ExactAlias,
    Fuzzy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenreMatch {
    pub canonical_name: String,
    pub kind: MatchKind,
    pub score: f64,
    pub primary_path: Vec<String>,
    pub paths: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenreResolution {
    pub input: String,
    pub normalized_input: String,
    pub matches: Vec<GenreMatch>,
    pub unresolved_segments: Vec<String>,
}

impl GenreResolution {
    pub fn is_clear(&self) -> bool {
        self.canonical_name().is_some()
    }

    pub fn canonical_name(&self) -> Option<&str> {
        if !self.unresolved_segments.is_empty() || self.matches.is_empty() {
            return None;
        }

        let mut names = self.matches.iter().map(|item| item.canonical_name.as_str());
        let Some(first) = names.next() else {
            return None;
        };

        if names.all(|name| name == first) {
            Some(first)
        } else {
            None
        }
    }

    pub fn is_ambiguous(&self) -> bool {
        if self.matches.is_empty() {
            return false;
        }

        if !self.unresolved_segments.is_empty() {
            return true;
        }

        let mut names = BTreeSet::new();
        for item in &self.matches {
            names.insert(item.canonical_name.clone());
        }
        names.len() > 1
    }
}

#[derive(Debug, Clone)]
pub struct GenreCatalog {
    entries: Vec<GenreEntry>,
    exact_lookup: HashMap<String, String>,
    alias_lookup: HashMap<String, String>,
    canonical_keys: Vec<String>,
}

impl GenreCatalog {
    pub fn from_embedded() -> Self {
        let taxonomy: TaxonomyNode =
            serde_json::from_str(TAXONOMY_JSON).expect("embedded taxonomy.json must be valid");
        let raw_aliases: HashMap<String, String> =
            serde_json::from_str(ALIASES_JSON).expect("embedded aliases.json must be valid");

        let mut paths_by_name: BTreeMap<String, BTreeSet<Vec<String>>> = BTreeMap::new();
        collect_paths(&taxonomy, &mut Vec::new(), &mut paths_by_name);

        let mut exact_candidates: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for name in paths_by_name.keys() {
            exact_candidates
                .entry(normalize_key(name))
                .or_default()
                .insert(name.clone());
        }

        let mut alias_candidates: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (alias, canonical) in raw_aliases {
            alias_candidates
                .entry(normalize_key(&alias))
                .or_default()
                .insert(canonical);
        }

        let exact_lookup = exact_candidates
            .into_iter()
            .filter_map(|(key, names)| {
                if names.len() == 1 {
                    Some((key, names.into_iter().next().expect("single element")))
                } else {
                    None
                }
            })
            .collect::<HashMap<_, _>>();

        let alias_lookup = alias_candidates
            .into_iter()
            .filter_map(|(key, names)| {
                if names.len() == 1 {
                    Some((key, names.into_iter().next().expect("single element")))
                } else {
                    None
                }
            })
            .collect::<HashMap<_, _>>();

        let mut entries = Vec::with_capacity(paths_by_name.len());
        for (name, paths) in paths_by_name {
            let mut paths = paths.into_iter().collect::<Vec<_>>();
            paths.sort_by(|left, right| {
                left.len()
                    .cmp(&right.len())
                    .then_with(|| path_string(left).cmp(&path_string(right)))
            });

            entries.push(GenreEntry {
                slug: slugify(&name),
                name,
                paths,
            });
        }

        let canonical_keys = entries.iter().map(|entry| entry.name.clone()).collect();

        Self {
            entries,
            exact_lookup,
            alias_lookup,
            canonical_keys,
        }
    }

    pub fn canonical_names(&self) -> &[String] {
        &self.canonical_keys
    }

    pub fn entry(&self, name: &str) -> Option<&GenreEntry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    pub fn path_for(&self, name: &str) -> Option<&[String]> {
        self.entry(name)
            .and_then(|entry| entry.paths.first().map(Vec::as_slice))
    }

    pub fn paths_for(&self, name: &str) -> Option<&[Vec<String>]> {
        self.entry(name).map(|entry| entry.paths.as_slice())
    }

    pub fn descendants_of(&self, name: &str) -> Vec<String> {
        let Some(target) = self.entry(name) else {
            return Vec::new();
        };

        let mut descendants = BTreeSet::new();
        for candidate in &self.entries {
            if candidate.name == target.name {
                continue;
            }

            let is_descendant = candidate.paths.iter().any(|candidate_path| {
                target
                    .paths
                    .iter()
                    .any(|target_path| is_descendant_path(target_path, candidate_path))
            });

            if is_descendant {
                descendants.insert(candidate.name.clone());
            }
        }

        descendants.into_iter().collect()
    }

    pub fn resolve(&self, raw: &str) -> GenreResolution {
        let input = raw.trim().to_string();
        let normalized_input = normalize_key(&input);
        let mut matches = Vec::new();
        let mut unresolved_segments = Vec::new();

        let segments = split_compound_terms(&input);
        if segments.len() > 1 {
            for segment in segments {
                match self.resolve_single(segment) {
                    Some(item) => matches.push(item),
                    None => unresolved_segments.push(segment.to_string()),
                }
            }

            return GenreResolution {
                input,
                normalized_input,
                matches,
                unresolved_segments,
            };
        }

        if let Some(single) = self.resolve_single(&input) {
            matches.push(single);
        } else if !input.is_empty() {
            unresolved_segments.push(input.clone());
        }

        GenreResolution {
            input,
            normalized_input,
            matches,
            unresolved_segments,
        }
    }

    pub fn resolve_single(&self, raw: &str) -> Option<GenreMatch> {
        let normalized = normalize_key(raw);
        if normalized.is_empty() {
            return None;
        }

        if let Some(canonical) = self.exact_lookup.get(&normalized) {
            if let Some(m) = self.match_for_name(canonical, MatchKind::ExactCanonical, 1.0) {
                return Some(m);
            }
        }

        if let Some(canonical) = self.alias_lookup.get(&normalized) {
            if let Some(m) = self.match_for_name(canonical, MatchKind::ExactAlias, 1.0) {
                return Some(m);
            }
        }

        self.best_fuzzy_match(&normalized)
    }

    fn match_for_name(&self, canonical_name: &str, kind: MatchKind, score: f64) -> Option<GenreMatch> {
        let entry = self.entry(canonical_name)?;
        let primary_path = entry
            .paths
            .iter()
            .max_by(|left, right| {
                left.len()
                    .cmp(&right.len())
                    .then_with(|| path_string(left).cmp(&path_string(right)))
            })
            .cloned()
            .unwrap_or_default();

        Some(GenreMatch {
            canonical_name: entry.name.clone(),
            kind,
            score,
            primary_path,
            paths: entry.paths.clone(),
        })
    }

    fn best_fuzzy_match(&self, normalized: &str) -> Option<GenreMatch> {
        if normalized.len() < 4 {
            return None;
        }

        let input_tokens: Vec<&str> = normalized.split_whitespace().collect();

        let mut scored = self
            .canonical_keys
            .iter()
            .map(|candidate| {
                let candidate_norm = normalize_key(candidate);
                let score = strsim::jaro_winkler(normalized, &candidate_norm);
                (candidate, candidate_norm, score)
            })
            .collect::<Vec<_>>();

        scored.sort_by(|(left_name, _, left_score), (right_name, _, right_score)| {
            right_score
                .total_cmp(left_score)
                .then_with(|| left_name.cmp(right_name))
        });

        let Some((best_name, best_norm, best_score)) = scored.first() else {
            return None;
        };

        // Raised from 0.90 to reduce false positives (e.g. "british" → "britpop").
        if *best_score < 0.92 {
            return None;
        }

        // Require at least one shared token to avoid purely character-level matches.
        let candidate_tokens: Vec<&str> = best_norm.split_whitespace().collect();
        let shares_token = input_tokens
            .iter()
            .any(|t| candidate_tokens.contains(t));
        if !shares_token {
            return None;
        }

        if let Some((second_name, _, second_score)) = scored.get(1) {
            if (best_score - second_score) < 0.05 {
                return None;
            }

            if (*best_score - *second_score).abs() < f64::EPSILON && best_name != second_name {
                return None;
            }
        }

        self.match_for_name(best_name, MatchKind::Fuzzy, *best_score)
    }
}

pub fn normalize_key(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}

pub fn split_compound_terms(value: &str) -> Vec<&str> {
    value
        .split(|ch| matches!(ch, ',' | ';' | '|' | '/'))
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn collect_paths(
    node: &TaxonomyNode,
    path: &mut Vec<String>,
    paths_by_name: &mut BTreeMap<String, BTreeSet<Vec<String>>>,
) {
    if node.name != "Genres" {
        path.push(node.name.clone());
        paths_by_name
            .entry(node.name.clone())
            .or_default()
            .insert(path.clone());
    }

    for child in &node.children {
        collect_paths(child, path, paths_by_name);
    }

    if node.name != "Genres" {
        path.pop();
    }
}

fn is_descendant_path(target: &[String], candidate: &[String]) -> bool {
    candidate.len() > target.len()
        && candidate
            .iter()
            .zip(target.iter())
            .all(|(left, right)| left == right)
}

fn path_string(path: &[String]) -> String {
    path.join(" > ")
}

#[cfg(test)]
mod tests {
    use super::{GenreCatalog, slugify};

    #[test]
    fn slugify_normalizes_names() {
        assert_eq!(slugify("Drum and Bass"), "drum-and-bass");
        assert_eq!(slugify("Post-Punk"), "post-punk");
        assert_eq!(slugify("Hi-NRG"), "hi-nrg");
    }

    #[test]
    fn catalog_contains_canonical_names_and_paths() {
        let catalog = GenreCatalog::from_embedded();
        assert!(catalog.canonical_names().contains(&"House".to_string()));
        assert!(catalog.paths_for("House").is_some());
        assert!(
            catalog
                .descendants_of("Electronic")
                .contains(&"House".to_string())
        );
    }
}
