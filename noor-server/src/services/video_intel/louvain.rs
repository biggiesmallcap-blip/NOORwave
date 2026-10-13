//! Louvain community detection over a weighted undirected graph in CSR
//! form. Deterministic: nodes are visited in index order (callers index
//! artists by sorted TIDAL id), ties go to the current community and then to
//! the lowest community index, and every sum runs in a fixed order, so the
//! same graph always gives the same partition.
//!
//! The resolution parameter scales the null model: above 1.0 favours more,
//! smaller communities (the scene finder splits giants at 2.0).

/// A move must beat staying put by more than this to count, so float noise
/// never makes a node oscillate between two equal communities.
const GAIN_EPSILON: f64 = 1e-12;
/// Safety stop for a local-moving phase; real graphs settle in a handful.
const MAX_PASSES: usize = 64;
/// Safety stop for the aggregate-and-repeat loop.
const MAX_LEVELS: usize = 32;

/// Weighted undirected graph in compressed sparse row form. An edge u-v with
/// u != v is stored in both rows. A self-loop is stored once and counts its
/// full weight toward the node's degree: the aggregation convention, where a
/// community's self-loop holds twice its internal edge weight.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Csr {
    pub offsets: Vec<usize>,
    pub targets: Vec<u32>,
    pub weights: Vec<f64>,
}

impl Csr {
    /// Undirected edges over nodes `0..n`, each pair listed once in either
    /// direction. Repeated pairs add up; non-positive or non-finite weights
    /// and out-of-range nodes are dropped.
    pub fn from_edges(n: usize, edges: &[(u32, u32, f64)]) -> Self {
        let mut entries = Vec::with_capacity(edges.len() * 2);
        for &(u, v, w) in edges {
            if !(w.is_finite() && w > 0.0) || u as usize >= n || v as usize >= n {
                continue;
            }
            entries.push((u, v, w));
            if u != v {
                entries.push((v, u, w));
            }
        }
        Self::from_entries(n, entries)
    }

    /// Row entries as given (no mirroring); used by aggregation, where each
    /// entry already is one direction of an undirected edge.
    fn from_entries(n: usize, mut entries: Vec<(u32, u32, f64)>) -> Self {
        // Stable, so repeated pairs add up in input order: same sums, same bits.
        entries.sort_by_key(|entry| (entry.0, entry.1));
        let mut offsets = vec![0usize; n + 1];
        let mut targets = Vec::with_capacity(entries.len());
        let mut weights: Vec<f64> = Vec::with_capacity(entries.len());
        let mut last: Option<(u32, u32)> = None;
        for (u, v, w) in entries {
            if last == Some((u, v)) {
                if let Some(total) = weights.last_mut() {
                    *total += w;
                }
                continue;
            }
            last = Some((u, v));
            offsets[u as usize + 1] += 1;
            targets.push(v);
            weights.push(w);
        }
        for i in 0..n {
            offsets[i + 1] += offsets[i];
        }
        Self {
            offsets,
            targets,
            weights,
        }
    }

    pub fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn row(&self, node: usize) -> (&[u32], &[f64]) {
        let (start, end) = (self.offsets[node], self.offsets[node + 1]);
        (&self.targets[start..end], &self.weights[start..end])
    }

    pub fn degree(&self, node: usize) -> f64 {
        self.row(node).1.iter().sum()
    }

    /// The subgraph induced by `nodes` (ascending), reindexed 0..len.
    pub fn induced(&self, nodes: &[u32]) -> Self {
        let mut local = vec![u32::MAX; self.len()];
        for (index, &node) in nodes.iter().enumerate() {
            local[node as usize] = index as u32;
        }
        let mut entries = Vec::new();
        for (index, &node) in nodes.iter().enumerate() {
            let (targets, weights) = self.row(node as usize);
            for (&target, &weight) in targets.iter().zip(weights) {
                let mapped = local[target as usize];
                if mapped != u32::MAX {
                    entries.push((index as u32, mapped, weight));
                }
            }
        }
        Self::from_entries(nodes.len(), entries)
    }
}

/// Community index per node, dense from 0 in order of each community's
/// lowest node.
pub fn louvain(graph: &Csr, resolution: f64) -> Vec<u32> {
    if graph.is_empty() {
        return Vec::new();
    }
    let n = graph.len();
    let mut membership: Vec<u32> = (0..n as u32).collect();
    let mut level = graph.clone();
    for _ in 0..MAX_LEVELS {
        let (local, moved) = local_moving(&level, resolution);
        if !moved {
            break;
        }
        for community in membership.iter_mut() {
            *community = local[*community as usize];
        }
        let count = local.iter().map(|c| *c as usize + 1).max().unwrap_or(0);
        if count == level.len() {
            break;
        }
        level = aggregate(&level, &local, count);
    }
    renumber(&membership)
}

/// Weighted modularity of a partition at a resolution.
pub fn modularity(graph: &Csr, partition: &[u32], resolution: f64) -> f64 {
    let communities = partition.iter().map(|c| *c as usize + 1).max().unwrap_or(0);
    let mut internal = vec![0.0; communities];
    let mut total = vec![0.0; communities];
    let mut two_m = 0.0;
    for node in 0..graph.len() {
        let community = partition[node] as usize;
        let (targets, weights) = graph.row(node);
        for (&target, &weight) in targets.iter().zip(weights) {
            two_m += weight;
            total[community] += weight;
            if partition[target as usize] as usize == community {
                internal[community] += weight;
            }
        }
    }
    if two_m <= 0.0 {
        return 0.0;
    }
    internal
        .iter()
        .zip(&total)
        .map(|(inside, tot)| inside / two_m - resolution * (tot / two_m).powi(2))
        .sum()
}

/// One phase of greedy moves. Returns the dense partition and whether any
/// node left its singleton.
fn local_moving(graph: &Csr, resolution: f64) -> (Vec<u32>, bool) {
    let n = graph.len();
    let degree: Vec<f64> = (0..n).map(|node| graph.degree(node)).collect();
    let two_m: f64 = degree.iter().sum();
    let mut community: Vec<u32> = (0..n as u32).collect();
    if two_m <= 0.0 {
        return (community, false);
    }
    let mut total = degree.clone();
    let mut link = vec![0.0f64; n];
    let mut stamp = vec![u32::MAX; n];
    let mut touched: Vec<u32> = Vec::new();
    let mut moved_any = false;
    for pass in 0..MAX_PASSES {
        let mut moves = 0usize;
        for node in 0..n {
            let current = community[node];
            let k = degree[node];
            touched.clear();
            let (targets, weights) = graph.row(node);
            for (&target, &weight) in targets.iter().zip(weights) {
                if target as usize == node {
                    continue;
                }
                let c = community[target as usize];
                if stamp[c as usize] != node as u32 {
                    stamp[c as usize] = node as u32;
                    link[c as usize] = 0.0;
                    touched.push(c);
                }
                link[c as usize] += weight;
            }
            total[current as usize] -= k;
            let gain = |c: u32, link_to: f64| link_to - resolution * total[c as usize] * k / two_m;
            let own_link = if stamp[current as usize] == node as u32 {
                link[current as usize]
            } else {
                0.0
            };
            let mut best = current;
            let mut best_gain = gain(current, own_link);
            for &c in &touched {
                if c == current {
                    continue;
                }
                let candidate = gain(c, link[c as usize]);
                if candidate > best_gain + GAIN_EPSILON {
                    best = c;
                    best_gain = candidate;
                } else if best != current
                    && (candidate - best_gain).abs() <= GAIN_EPSILON
                    && c < best
                {
                    best = c;
                }
            }
            total[best as usize] += k;
            if best != current {
                community[node] = best;
                moves += 1;
            }
            // Stamps are per node; reset so the next node starts clean even
            // when indices collide across passes.
            for &c in &touched {
                stamp[c as usize] = u32::MAX;
            }
        }
        if moves == 0 {
            break;
        }
        moved_any = true;
        if pass + 1 == MAX_PASSES {
            break;
        }
    }
    (renumber(&community), moved_any)
}

/// Collapse each community into one node; the new self-loop holds every
/// internal entry (twice the internal weight plus old self-loops).
fn aggregate(graph: &Csr, partition: &[u32], count: usize) -> Csr {
    let mut entries = Vec::with_capacity(graph.targets.len());
    for node in 0..graph.len() {
        let from = partition[node];
        let (targets, weights) = graph.row(node);
        for (&target, &weight) in targets.iter().zip(weights) {
            entries.push((from, partition[target as usize], weight));
        }
    }
    Csr::from_entries(count, entries)
}

/// Dense labels in order of first appearance.
fn renumber(labels: &[u32]) -> Vec<u32> {
    let mut map = std::collections::HashMap::new();
    labels
        .iter()
        .map(|label| {
            let next = map.len() as u32;
            *map.entry(*label).or_insert(next)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clique(start: u32, size: u32, weight: f64) -> Vec<(u32, u32, f64)> {
        let mut edges = Vec::new();
        for a in start..start + size {
            for b in a + 1..start + size {
                edges.push((a, b, weight));
            }
        }
        edges
    }

    fn groups(partition: &[u32]) -> Vec<Vec<usize>> {
        let count = partition.iter().map(|c| *c as usize + 1).max().unwrap_or(0);
        let mut out = vec![Vec::new(); count];
        for (node, c) in partition.iter().enumerate() {
            out[*c as usize].push(node);
        }
        out
    }

    #[test]
    fn csr_mirrors_edges_and_merges_repeats() {
        let csr = Csr::from_edges(3, &[(0, 1, 0.5), (1, 0, 0.25), (1, 2, 1.0), (2, 2, 0.4)]);
        assert_eq!(csr.row(0).0, &[1]);
        assert!((csr.row(0).1[0] - 0.75).abs() < 1e-12);
        assert_eq!(csr.row(1).0, &[0, 2]);
        assert_eq!(csr.row(2).0, &[1, 2]);
        assert!((csr.degree(2) - 1.4).abs() < 1e-12);
        let dropped = Csr::from_edges(2, &[(0, 1, 0.0), (0, 5, 1.0), (0, 1, f64::NAN)]);
        assert!(dropped.targets.is_empty());
    }

    #[test]
    fn two_cliques_joined_by_a_weak_edge_split_in_two() {
        let mut edges = clique(0, 5, 1.0);
        edges.extend(clique(5, 5, 1.0));
        edges.push((4, 5, 0.1));
        let csr = Csr::from_edges(10, &edges);
        let partition = louvain(&csr, 1.0);
        assert_eq!(
            groups(&partition),
            vec![vec![0, 1, 2, 3, 4], vec![5, 6, 7, 8, 9]]
        );
    }

    #[test]
    fn a_ring_of_cliques_finds_every_clique() {
        let mut edges = Vec::new();
        for ring in 0..8u32 {
            edges.extend(clique(ring * 6, 6, 1.0));
            edges.push((ring * 6, ((ring + 1) % 8) * 6 + 1, 1.0));
        }
        let csr = Csr::from_edges(48, &edges);
        let partition = louvain(&csr, 1.0);
        let found = groups(&partition);
        assert_eq!(found.len(), 8);
        for (index, group) in found.iter().enumerate() {
            let expected: Vec<usize> = (index * 6..index * 6 + 6).collect();
            assert_eq!(group, &expected);
        }
        let singletons: Vec<u32> = (0..48).collect();
        assert!(modularity(&csr, &partition, 1.0) > modularity(&csr, &singletons, 1.0));
    }

    #[test]
    fn resolution_controls_granularity() {
        // Two cliques with a strong bridge: one community when the null model
        // is weak, two at the usual resolution, more when it is strong.
        let mut edges = clique(0, 4, 1.0);
        edges.extend(clique(4, 4, 1.0));
        edges.extend([(0, 4, 1.0), (1, 5, 1.0), (2, 6, 1.0)]);
        let csr = Csr::from_edges(8, &edges);
        assert_eq!(groups(&louvain(&csr, 0.05)).len(), 1);
        assert_eq!(groups(&louvain(&csr, 1.0)).len(), 2);
        assert!(groups(&louvain(&csr, 6.0)).len() > 2);
    }

    #[test]
    fn louvain_is_deterministic_and_handles_empty_graphs() {
        let mut edges = Vec::new();
        for i in 0..60u32 {
            edges.push((i, (i * 7 + 3) % 60, 0.3 + (i % 5) as f64 * 0.1));
            edges.push((i, (i * 13 + 1) % 60, 0.5));
        }
        let csr = Csr::from_edges(60, &edges);
        assert_eq!(louvain(&csr, 1.0), louvain(&csr, 1.0));
        assert_eq!(louvain(&Csr::from_edges(3, &[]), 1.0), vec![0, 1, 2]);
        assert!(louvain(&Csr::from_edges(0, &[]), 1.0).is_empty());
    }

    #[test]
    fn induced_subgraph_keeps_only_inside_edges() {
        let mut edges = clique(0, 3, 1.0);
        edges.push((2, 3, 0.5));
        let csr = Csr::from_edges(4, &edges);
        let sub = csr.induced(&[1, 2, 3]);
        assert_eq!(sub.len(), 3);
        assert_eq!(sub.row(0).0, &[1]);
        assert_eq!(sub.row(1).0, &[0, 2]);
        assert!((sub.degree(2) - 0.5).abs() < 1e-12);
    }
}
