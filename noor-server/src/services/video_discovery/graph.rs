//! Weighted artist graph and relevance propagation for video discovery.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::Connection;

/// Each hop past the first keeps 60% of the path's strength.
pub const HOP_DECAY: f64 = 0.6;
/// Below this an artist is never scheduled or offered to a station.
pub const RELEVANCE_FLOOR: f64 = 0.08;
pub const FEATURED_WEIGHT: f64 = 0.85;
const PATHS_PER_NODE: usize = 3;
const CACHE_TTL: Duration = Duration::from_secs(600);
const DIRTY_REBUILD_AFTER: Duration = Duration::from_secs(30);

pub fn tidal_weight(rank: usize) -> f64 {
    (1.0 - 0.04 * rank as f64).max(0.6)
}

pub fn lastfm_weight(match_score: Option<f64>, rank: usize) -> f64 {
    match match_score {
        Some(score) if score.is_finite() => score.clamp(0.3, 1.0),
        _ => (1.0 - 0.035 * rank as f64).max(0.3),
    }
}

/// Weight of a co-list edge seen in `1 + extra_lists` distinct curated lists.
pub fn colist_weight(extra_lists: i64) -> f64 {
    (0.4 + 0.1 * extra_lists.max(0) as f64).min(0.8)
}

/// Independent evidence combined: any one strong signal is enough, several
/// medium ones add up, nothing exceeds 1.
pub fn noisy_or(values: impl IntoIterator<Item = f64>) -> f64 {
    1.0 - values
        .into_iter()
        .fold(1.0, |acc, value| acc * (1.0 - value.clamp(0.0, 1.0)))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Relevance {
    pub score: f64,
    /// Fewest hops from any root at which the artist was reached.
    pub hops: u8,
}

#[derive(Debug, Default)]
pub struct Graph {
    adj: HashMap<i64, Vec<(i64, f64)>>,
}

impl Graph {
    pub fn from_edges(edges: impl IntoIterator<Item = (i64, i64, f64)>) -> Self {
        let mut combined: HashMap<(i64, i64), Vec<f64>> = HashMap::new();
        for (from, to, weight) in edges {
            if from > 0 && to > 0 && from != to {
                combined.entry((from, to)).or_default().push(weight);
            }
        }
        let mut adj: HashMap<i64, Vec<(i64, f64)>> = HashMap::new();
        for ((from, to), weights) in combined {
            adj.entry(from).or_default().push((to, noisy_or(weights)));
        }
        for list in adj.values_mut() {
            list.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        }
        Self { adj }
    }

    pub fn load(conn: &Connection) -> Result<Self> {
        let mut stmt = conn.prepare(
            "SELECT seed_tidal_id, related_tidal_id, weight FROM video_related_artists
              WHERE source IN ('tidal', 'lastfm', 'featured', 'colist')",
        )?;
        let edges = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<(i64, i64, f64)>, _>>()?;
        Ok(Self::from_edges(edges))
    }

    pub fn neighbors(&self, artist_id: i64) -> &[(i64, f64)] {
        self.adj.get(&artist_id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Spread relevance from weighted roots, layer by layer, up to `max_hops`.
    /// A node's score is noisy-OR over its best few incoming contributions.
    pub fn propagate(&self, roots: &[(i64, f64)], max_hops: u8) -> HashMap<i64, Relevance> {
        let mut best: HashMap<i64, Relevance> = HashMap::new();
        for &(id, weight) in roots {
            let score = weight.clamp(0.0, 1.0);
            if id <= 0 || score <= 0.0 {
                continue;
            }
            let entry = best.entry(id).or_insert(Relevance {
                score: 0.0,
                hops: 0,
            });
            entry.score = entry.score.max(score);
        }
        let mut frontier: Vec<(i64, f64)> = best.iter().map(|(id, r)| (*id, r.score)).collect();
        for hop in 1..=max_hops {
            let decay = if hop == 1 { 1.0 } else { HOP_DECAY };
            let mut incoming: HashMap<i64, Vec<f64>> = HashMap::new();
            for &(from, score) in &frontier {
                for &(to, weight) in self.neighbors(from) {
                    let contribution = score * weight * decay;
                    if contribution >= RELEVANCE_FLOOR / 2.0 {
                        incoming.entry(to).or_default().push(contribution);
                    }
                }
            }
            let mut next = Vec::new();
            for (id, mut contributions) in incoming {
                contributions.sort_by(|a, b| b.total_cmp(a));
                contributions.truncate(PATHS_PER_NODE);
                let score = noisy_or(contributions);
                match best.get_mut(&id) {
                    Some(existing) if existing.score >= score => continue,
                    Some(existing) => existing.score = score,
                    None => {
                        best.insert(id, Relevance { score, hops: hop });
                    }
                }
                if score >= RELEVANCE_FLOOR {
                    next.push((id, score));
                }
            }
            frontier = next;
        }
        best.retain(|_, relevance| relevance.score >= RELEVANCE_FLOOR);
        best
    }
}

static CACHE: Mutex<Option<(Instant, Arc<Graph>)>> = Mutex::new(None);
static DIRTY: AtomicBool = AtomicBool::new(false);

/// Edges changed. The next read after a short settle rebuilds the graph.
pub fn mark_dirty() {
    DIRTY.store(true, Ordering::Relaxed);
}

/// The shared graph, rebuilt every 10 minutes or 30 s after a change. Tests
/// each own a database, so they always read fresh.
pub fn cached(conn: &Connection) -> Result<Arc<Graph>> {
    if cfg!(test) {
        return Ok(Arc::new(Graph::load(conn)?));
    }
    if let Ok(cache) = CACHE.lock()
        && let Some((built, graph)) = cache.as_ref()
    {
        let age = built.elapsed();
        let dirty = DIRTY.load(Ordering::Relaxed);
        if age < CACHE_TTL && !(dirty && age >= DIRTY_REBUILD_AFTER) {
            return Ok(graph.clone());
        }
    }
    rebuild(conn)
}

/// Rebuild now if edges changed; used by urgent station work so a fresh
/// expansion is visible to the very next plan.
pub fn refresh_if_dirty(conn: &Connection) -> Result<Arc<Graph>> {
    if !cfg!(test) && DIRTY.load(Ordering::Relaxed) {
        return rebuild(conn);
    }
    cached(conn)
}

fn rebuild(conn: &Connection) -> Result<Arc<Graph>> {
    DIRTY.store(false, Ordering::Relaxed);
    let graph = Arc::new(Graph::load(conn)?);
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((Instant::now(), graph.clone()));
    }
    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn weights_follow_provider_rank_and_score() {
        assert!(close(tidal_weight(0), 1.0));
        assert!(close(tidal_weight(5), 0.8));
        assert!(close(tidal_weight(30), 0.6));
        assert!(close(lastfm_weight(Some(0.9), 3), 0.9));
        assert!(close(lastfm_weight(Some(0.05), 3), 0.3));
        assert!(close(lastfm_weight(None, 2), 0.93));
        assert!(close(colist_weight(0), 0.4));
        assert!(close(colist_weight(2), 0.6));
        assert!(close(colist_weight(9), 0.8));
        assert!(close(noisy_or([0.5, 0.5]), 0.75));
    }

    #[test]
    fn agreeing_providers_strengthen_one_edge() {
        let graph = Graph::from_edges([(1, 2, 0.6), (1, 2, 0.5), (2, 2, 1.0)]);
        assert_eq!(graph.neighbors(1).len(), 1);
        assert!(close(graph.neighbors(1)[0].1, 0.8));
        assert!(graph.neighbors(2).is_empty());
    }

    #[test]
    fn relevance_decays_per_hop_and_respects_depth() {
        let graph = Graph::from_edges([(1, 2, 0.9), (2, 3, 0.9), (3, 4, 0.9)]);
        let two = graph.propagate(&[(1, 1.0)], 2);
        assert_eq!(
            two[&1],
            Relevance {
                score: 1.0,
                hops: 0
            }
        );
        assert!(close(two[&2].score, 0.9));
        assert!(close(two[&3].score, 0.9 * 0.9 * 0.6));
        assert_eq!(two[&3].hops, 2);
        assert!(!two.contains_key(&4));
        let three = graph.propagate(&[(1, 1.0)], 3);
        assert!(three.contains_key(&4));
    }

    #[test]
    fn two_medium_paths_beat_one_and_weak_chains_fall_below_the_floor() {
        let graph = Graph::from_edges([
            (1, 2, 0.8),
            (1, 3, 0.8),
            (2, 9, 0.6),
            (3, 9, 0.6),
            (2, 8, 0.6),
            (1, 4, 0.3),
            (4, 7, 0.3),
        ]);
        let relevance = graph.propagate(&[(1, 1.0)], 2);
        assert!(relevance[&9].score > relevance[&8].score);
        assert!(
            !relevance.contains_key(&7),
            "0.3 x 0.3 x 0.6 is under the floor"
        );
    }

    #[test]
    fn a_direct_neighbor_keeps_hop_one_even_when_reached_again() {
        let graph = Graph::from_edges([(1, 2, 0.2), (1, 3, 1.0), (3, 2, 1.0)]);
        let relevance = graph.propagate(&[(1, 1.0)], 2);
        assert_eq!(relevance[&2].hops, 1);
        assert!(relevance[&2].score > 0.2);
    }
}
