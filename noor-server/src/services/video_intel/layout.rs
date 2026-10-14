//! Map layout: scene disks from a force layout of the scene graph, each
//! scene's artists from a force layout of its own internal edges, uncharted
//! artists on an outer ring, warm-started so nothing jumps between builds.
//!
//! The map is a 1000 x 1000 square, origin top-left. Scenes live inside a
//! circle of radius 400 around the centre; the uncharted ring runs from 425
//! to 470. A scene's footprint (area-equivalent radius) grows with
//! sqrt(videos); its artists form an organic cloud of about that area,
//! knotted where sub-communities are, eased (never cut) inside a bound of
//! `PACK` footprints, and that bound is the `r` scenes pack by, so clouds
//! never overlap. Hue is the angle of a scene's centre around the map
//! centre, so color is place.
//!
//! Warm start: when most scenes continue from the last build, those keep
//! their positions and only new scenes are simulated; a final overlap pass
//! moves anyone just enough to make room for growth. Inside a scene,
//! remembered artists stay where they were and newcomers settle among their
//! neighbours. On unchanged data a rebuild reproduces the previous map
//! exactly.

use std::collections::{BTreeMap, HashMap};
use std::f64::consts::{PI, TAU};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use super::louvain::{self, Csr};
use super::scenes::{self, ArtistGraph};
use crate::services::video_discovery::names::name_key;

pub const MAP_SIZE: f64 = 1000.0;
pub const CENTRE: f64 = MAP_SIZE / 2.0;
/// Every scene disk fits inside this radius around the centre.
pub const SCENE_FIELD: f64 = 400.0;
pub const RING_INNER: f64 = 425.0;
pub const RING_BAND: f64 = 45.0;
/// Gap between disks, in model units (radius = sqrt(videos)).
const GAP: f64 = 6.0;
const COLD_ITERATIONS: usize = 600;
const WARM_ITERATIONS: usize = 300;
const LINK_STRENGTH: f64 = 0.25;
const GRAVITY: f64 = 0.012;
const SETTLE_PASSES: usize = 800;
/// Tries at a cold map; the one whose linked scenes sit closest wins.
const COLD_ATTEMPTS: u64 = 8;
/// Links each scene keeps for the layout (the union over scenes).
const LINKS_PER_SCENE: usize = 6;
/// A scene's artists may reach this many footprints (the area-equivalent
/// radius from sqrt(videos)) from its centre; the scene's `r` on the map,
/// the radius scenes pack by, is this bound, so clouds never overlap.
pub const PACK: f64 = 1.12;
/// Share of the bound past which points are eased in rather than cut.
const SOFT_KNEE: f64 = 0.55;
const INNER_ITERATIONS: usize = 150;
/// Pull toward the scene centre per unit of distance, the pull of a
/// knot's centre on its members, and how hard knots push each other apart.
const INNER_GRAVITY: f64 = 0.08;
const KNOT_COHESION: f64 = 0.6;
const KNOT_REPULSION: f64 = 0.6;
/// Sub-communities smaller than this are loners, not knots.
const MIN_KNOT: usize = 3;
/// A newcomer's offset from its neighbours, as a share of the footprint
/// each artist would get.
const INNER_SPACING: f64 = 0.55;
/// Cloud reach (the radius three quarters of a scene's artists sit within,
/// in footprints) for sparse and densely linked scenes: tight scenes look
/// tight.
const INNER_REACH_SPARSE: f64 = 0.78;
const INNER_REACH_DENSE: f64 = 0.64;
/// Above this share of newcomers a scene relaxes as a whole.
const RESHAPE_SHARE: f64 = 0.25;
/// Marks which interior layout the stored positions came from; anything
/// else is laid out cold once.
const LAYOUT_VERSION_KEY: &str = "video_intel.layout_version";
const LAYOUT_VERSION: &str = "organic-1";

/// splitmix64: a fixed, well-mixed hash so positions never depend on the
/// standard library's hasher.
pub fn hash64(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Two uniform values in [0, 1) from an id.
fn unit_pair(id: i64) -> (f64, f64) {
    let h = hash64(id as u64);
    let a = (h >> 11) as f64 / (1u64 << 53) as f64;
    let b = (hash64(h) >> 11) as f64 / (1u64 << 53) as f64;
    (a, b)
}

/// Degrees 0..360 of a point around the map centre (y grows downward, so
/// angles run clockwise on screen).
pub fn hue_of(x: f64, y: f64) -> f64 {
    let degrees = (y - CENTRE).atan2(x - CENTRE).to_degrees();
    (degrees + 360.0) % 360.0
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneNode {
    pub id: i64,
    pub videos: i64,
    /// Previous map placement (x, y, r) when the scene continues one.
    pub prev: Option<(f64, f64, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub hue: f64,
}

/// Scene graph edge between node indices; weight already normalised.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneLink {
    pub a: usize,
    pub b: usize,
    pub weight: f64,
}

/// Inter-scene weight / sqrt(artists_a x artists_b), scaled so the strongest
/// link is 1.
pub fn scene_links(
    graph: &ArtistGraph,
    scene_index_of: &HashMap<i64, usize>,
    artists_per_scene: &[i64],
) -> Vec<SceneLink> {
    let mut sums: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    for (node, id) in graph.ids.iter().enumerate() {
        let Some(&a) = scene_index_of.get(id) else {
            continue;
        };
        let (targets, weights) = graph.csr.row(node);
        for (&target, &weight) in targets.iter().zip(weights) {
            if let Some(&b) = scene_index_of.get(&graph.ids[target as usize])
                && a < b
            {
                *sums.entry((a, b)).or_default() += weight;
            }
        }
    }
    let mut links: Vec<SceneLink> = sums
        .into_iter()
        .map(|((a, b), sum)| SceneLink {
            a,
            b,
            weight: sum
                / ((artists_per_scene[a].max(1) * artists_per_scene[b].max(1)) as f64).sqrt(),
        })
        .collect();
    let top = links.iter().map(|l| l.weight).fold(0.0, f64::max);
    if top > 0.0 {
        for link in &mut links {
            link.weight /= top;
        }
    }
    links
}

fn push_apart(
    pos: &mut [(f64, f64)],
    radius: &[f64],
    movable: &[bool],
    i: usize,
    j: usize,
    ids: &[i64],
) -> bool {
    let need = radius[i] + radius[j] + GAP;
    let (mut dx, mut dy) = (pos[j].0 - pos[i].0, pos[j].1 - pos[i].1);
    let mut d = (dx * dx + dy * dy).sqrt();
    if d >= need - 1e-9 {
        return false;
    }
    if d < 1e-9 {
        let (angle, _) = unit_pair(ids[i].wrapping_mul(31).wrapping_add(ids[j]));
        dx = (angle * TAU).cos();
        dy = (angle * TAU).sin();
        d = 1.0;
    } else {
        dx /= d;
        dy /= d;
    }
    let overlap = need - d.max(0.0);
    // Bigger disks give way less; a pinned disk does not give way.
    let (wi, wj) = match (movable[i], movable[j]) {
        (true, true) => {
            let total = radius[i] + radius[j];
            (radius[j] / total, radius[i] / total)
        }
        (true, false) => (1.0, 0.0),
        (false, true) => (0.0, 1.0),
        (false, false) => return false,
    };
    pos[i].0 -= dx * overlap * wi;
    pos[i].1 -= dy * overlap * wi;
    pos[j].0 += dx * overlap * wj;
    pos[j].1 += dy * overlap * wj;
    true
}

/// Each scene's strongest few links (their union): the scene graph is nearly
/// complete, and a hundred faint pulls only blur the few that matter.
fn strongest_links(links: &[SceneLink], n: usize) -> Vec<SceneLink> {
    let mut per_scene: Vec<Vec<(f64, usize)>> = vec![Vec::new(); n];
    for (index, link) in links.iter().enumerate() {
        if link.a < n && link.b < n && link.a != link.b && link.weight > 0.0 {
            per_scene[link.a].push((link.weight, index));
            per_scene[link.b].push((link.weight, index));
        }
    }
    let mut keep = vec![false; links.len()];
    for list in &mut per_scene {
        list.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        for (_, index) in list.iter().take(LINKS_PER_SCENE) {
            keep[*index] = true;
        }
    }
    links
        .iter()
        .zip(keep)
        .filter(|(_, kept)| *kept)
        .map(|(link, _)| *link)
        .collect()
}

/// Grow the map from the biggest scene: each next scene is the one most
/// strongly linked to those already placed, set down at their weighted
/// centre (nudged by a hashed angle); unlinked scenes ring the outside.
fn greedy_start(
    order: &[usize],
    radius: &[f64],
    links: &[SceneLink],
    ids: &[i64],
) -> Vec<(f64, f64)> {
    let n = order.len();
    let mut pos = vec![(0.0, 0.0); n];
    let mut placed = vec![false; n];
    let mut pull = vec![0.0f64; n];
    let mut extent: f64 = 0.0;
    for step in 0..n {
        let next = order
            .iter()
            .copied()
            .filter(|i| !placed[*i])
            .fold(None, |best: Option<usize>, i| match best {
                Some(b) if pull[b] >= pull[i] => Some(b),
                _ => Some(i),
            })
            .unwrap_or(order[step]);
        let (angle, _) = unit_pair(ids[next]);
        let (sx, sy, total) = links.iter().fold((0.0, 0.0, 0.0), |acc, link| {
            let other = if link.a == next {
                link.b
            } else if link.b == next {
                link.a
            } else {
                return acc;
            };
            if placed[other] {
                (
                    acc.0 + pos[other].0 * link.weight,
                    acc.1 + pos[other].1 * link.weight,
                    acc.2 + link.weight,
                )
            } else {
                acc
            }
        });
        pos[next] = if step == 0 {
            (0.0, 0.0)
        } else if total > 0.0 {
            (
                sx / total + (angle * TAU).cos() * radius[next],
                sy / total + (angle * TAU).sin() * radius[next],
            )
        } else {
            let out = extent + radius[next];
            (out * (angle * TAU).cos(), out * (angle * TAU).sin())
        };
        placed[next] = true;
        extent = extent.max((pos[next].0.powi(2) + pos[next].1.powi(2)).sqrt() + radius[next]);
        for link in links {
            if link.a == next {
                pull[link.b] += link.weight;
            } else if link.b == next {
                pull[link.a] += link.weight;
            }
        }
    }
    pos
}

/// Springs on links (rest length: the disks touching), mild gravity,
/// overlap pushes; cooling linearly. Only movable disks move.
fn simulate(
    pos: &mut [(f64, f64)],
    radius: &[f64],
    movable: &[bool],
    links: &[SceneLink],
    iterations: usize,
    ids: &[i64],
) {
    let n = pos.len();
    for step in 0..iterations {
        let alpha = (1.0 - step as f64 / iterations as f64).max(0.02);
        for link in links {
            let (a, b) = (link.a, link.b);
            if !movable[a] && !movable[b] {
                continue;
            }
            let (dx, dy) = (pos[b].0 - pos[a].0, pos[b].1 - pos[a].1);
            let d = (dx * dx + dy * dy).sqrt().max(1e-9);
            let target = radius[a] + radius[b] + GAP;
            let k = (d - target) / d * link.weight * alpha * LINK_STRENGTH;
            let (wa, wb) = match (movable[a], movable[b]) {
                (true, true) => {
                    let total = radius[a] + radius[b];
                    (radius[b] / total, radius[a] / total)
                }
                (true, false) => (1.0, 0.0),
                _ => (0.0, 1.0),
            };
            pos[a].0 += dx * k * wa;
            pos[a].1 += dy * k * wa;
            pos[b].0 -= dx * k * wb;
            pos[b].1 -= dy * k * wb;
        }
        for i in 0..n {
            if movable[i] {
                pos[i].0 -= pos[i].0 * GRAVITY * alpha;
                pos[i].1 -= pos[i].1 * GRAVITY * alpha;
            }
        }
        for i in 0..n {
            for j in i + 1..n {
                push_apart(pos, radius, movable, i, j, ids);
            }
        }
    }
}

/// Everyone may move, just enough to clear overlaps.
fn settle_overlaps(pos: &mut [(f64, f64)], radius: &[f64], ids: &[i64]) {
    let n = pos.len();
    let everyone = vec![true; n];
    for _ in 0..SETTLE_PASSES {
        let mut moved = false;
        for i in 0..n {
            for j in i + 1..n {
                moved |= push_apart(pos, radius, &everyone, i, j, ids);
            }
        }
        if !moved {
            break;
        }
    }
}

/// Weighted link length relative to the map's size: lower means linked
/// scenes sit closer together.
fn link_length(pos: &[(f64, f64)], radius: &[f64], links: &[SceneLink]) -> f64 {
    let n = pos.len().max(1) as f64;
    let (cx, cy) = pos
        .iter()
        .fold((0.0, 0.0), |acc, p| (acc.0 + p.0 / n, acc.1 + p.1 / n));
    let extent = pos
        .iter()
        .zip(radius)
        .map(|(p, r)| ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt() + r)
        .fold(0.0, f64::max)
        .max(1e-9);
    links
        .iter()
        .map(|link| {
            let (a, b) = (pos[link.a], pos[link.b]);
            link.weight * ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
        })
        .sum::<f64>()
        / extent
}

/// Place scene disks. Pure and deterministic.
pub fn place_scenes(nodes: &[SceneNode], links: &[SceneLink]) -> Vec<Placement> {
    let n = nodes.len();
    if n == 0 {
        return Vec::new();
    }
    let links = strongest_links(links, n);
    let ids: Vec<i64> = nodes.iter().map(|node| node.id).collect();
    // Disks are the scenes' bounds: PACK footprints of sqrt(videos).
    let radius: Vec<f64> = nodes
        .iter()
        .map(|node| (node.videos.max(1) as f64).sqrt() * PACK)
        .collect();
    let inherited: Vec<bool> = nodes
        .iter()
        .map(|node| node.prev.is_some_and(|(_, _, r)| r > 0.0))
        .collect();
    let kept = inherited.iter().filter(|k| **k).count();
    let warm = kept > 0 && kept * 2 >= n;
    // Previous map scale: map radius per model radius.
    let mut ratios: Vec<f64> = nodes
        .iter()
        .zip(&radius)
        .filter_map(|(node, r)| node.prev.filter(|p| p.2 > 0.0).map(|p| p.2 / r))
        .collect();
    ratios.sort_by(f64::total_cmp);
    let prev_scale = ratios.get(ratios.len() / 2).copied().unwrap_or(1.0);

    let mut pos = vec![(0.0f64, 0.0f64); n];
    // Biggest first; a golden-angle spiral gives a compact start.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| {
        nodes[*b]
            .videos
            .cmp(&nodes[*a].videos)
            .then(ids[*a].cmp(&ids[*b]))
    });
    let mean_r = radius.iter().sum::<f64>() / n as f64;
    let spiral = |slot: usize| {
        let angle = slot as f64 * 2.399_963_229_728_653;
        let dist = (slot as f64 + 0.5).sqrt() * mean_r * 2.2;
        (dist * angle.cos(), dist * angle.sin())
    };
    let movable: Vec<bool> = (0..n).map(|i| !(warm && inherited[i])).collect();
    if warm {
        for i in 0..n {
            if let Some((x, y, _)) = nodes[i].prev.filter(|_| inherited[i]) {
                pos[i] = ((x - CENTRE) / prev_scale, (y - CENTRE) / prev_scale);
            }
        }
        let extent = (0..n)
            .filter(|i| inherited[*i])
            .map(|i| (pos[i].0.powi(2) + pos[i].1.powi(2)).sqrt() + radius[i])
            .fold(0.0, f64::max);
        for (slot, &i) in order.iter().enumerate() {
            if inherited[i] {
                continue;
            }
            // Next to the kept scenes it links to, else just outside the map.
            let (mut sx, mut sy, mut total) = (0.0, 0.0, 0.0);
            for link in &links {
                let other = if link.a == i {
                    link.b
                } else if link.b == i {
                    link.a
                } else {
                    continue;
                };
                if inherited[other] {
                    sx += pos[other].0 * link.weight;
                    sy += pos[other].1 * link.weight;
                    total += link.weight;
                }
            }
            let (jitter, _) = unit_pair(ids[i]);
            pos[i] = if total > 0.0 {
                (
                    sx / total + (jitter * TAU).cos() * radius[i],
                    sy / total + (jitter * TAU).sin() * radius[i],
                )
            } else {
                let angle = jitter * TAU + slot as f64;
                (
                    (extent + radius[i]) * angle.cos(),
                    (extent + radius[i]) * angle.sin(),
                )
            };
        }
        if movable.iter().any(|m| *m) {
            simulate(&mut pos, &radius, &movable, &links, WARM_ITERATIONS, &ids);
        }
        settle_overlaps(&mut pos, &radius, &ids);
    } else {
        // A cold map is drawn once and then kept for good, so it is worth a
        // few tries: a greedy start that grows the map along its strongest
        // links, then shuffled spirals; the try whose linked scenes sit
        // closest (relative to the map's size) wins.
        let everyone = vec![true; n];
        let mut best: Option<(f64, Vec<(f64, f64)>)> = None;
        for attempt in 0..COLD_ATTEMPTS {
            let mut trial = if attempt == 0 {
                greedy_start(&order, &radius, &links, &ids)
            } else {
                let mut shuffled = order.clone();
                shuffled.sort_by_key(|i| hash64((ids[*i] as u64) ^ hash64(attempt)));
                let mut trial = vec![(0.0, 0.0); n];
                for (slot, &i) in shuffled.iter().enumerate() {
                    trial[i] = spiral(slot);
                }
                trial
            };
            simulate(
                &mut trial,
                &radius,
                &everyone,
                &links,
                COLD_ITERATIONS,
                &ids,
            );
            settle_overlaps(&mut trial, &radius, &ids);
            let score = link_length(&trial, &radius, &links);
            if best.as_ref().is_none_or(|(s, _)| score < *s) {
                best = Some((score, trial));
            }
        }
        if let Some((_, winner)) = best {
            pos = winner;
        }
    }

    let (offset, scale) = if warm {
        let extent = (0..n)
            .map(|i| (pos[i].0.powi(2) + pos[i].1.powi(2)).sqrt() + radius[i])
            .fold(0.0, f64::max);
        let scale = if extent * prev_scale > SCENE_FIELD {
            SCENE_FIELD / extent
        } else {
            prev_scale
        };
        ((0.0, 0.0), scale)
    } else {
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for i in 0..n {
            min_x = min_x.min(pos[i].0 - radius[i]);
            max_x = max_x.max(pos[i].0 + radius[i]);
            min_y = min_y.min(pos[i].1 - radius[i]);
            max_y = max_y.max(pos[i].1 + radius[i]);
        }
        // Badoiu-Clarkson: walk the centre toward the farthest disk edge
        // with shrinking steps; converges on the smallest enclosing circle,
        // so an irregular blob sits centred in the field.
        let mut offset = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
        for step in 1..=400 {
            let reach = |i: usize| {
                ((pos[i].0 - offset.0).powi(2) + (pos[i].1 - offset.1).powi(2)).sqrt() + radius[i]
            };
            let far = (0..n).fold(0, |best, i| if reach(i) > reach(best) { i } else { best });
            let (dx, dy) = (pos[far].0 - offset.0, pos[far].1 - offset.1);
            let d = (dx * dx + dy * dy).sqrt();
            let edge = if d > 1e-9 {
                (
                    pos[far].0 + dx / d * radius[far],
                    pos[far].1 + dy / d * radius[far],
                )
            } else {
                (pos[far].0 + radius[far], pos[far].1)
            };
            offset.0 += (edge.0 - offset.0) / (step as f64 + 1.0);
            offset.1 += (edge.1 - offset.1) / (step as f64 + 1.0);
        }
        let extent = (0..n)
            .map(|i| {
                ((pos[i].0 - offset.0).powi(2) + (pos[i].1 - offset.1).powi(2)).sqrt() + radius[i]
            })
            .fold(0.0, f64::max);
        (offset, SCENE_FIELD / extent.max(1e-9))
    };
    (0..n)
        .map(|i| {
            let x = CENTRE + (pos[i].0 - offset.0) * scale;
            let y = CENTRE + (pos[i].1 - offset.1) * scale;
            Placement {
                x,
                y,
                r: radius[i] * scale,
                hue: hue_of(x, y),
            }
        })
        .collect()
}

// --------------------------------------------------------------- scene interiors

/// Ordinal edge strength: provider weights pile up at a few values, so only
/// their order within the scene counts, spread over 0.3..1.
fn ordinal_links(edges: &[(usize, usize, f64)], n: usize) -> Vec<(usize, usize, f64)> {
    let mut levels: Vec<f64> = edges.iter().map(|e| e.2).collect();
    levels.sort_by(f64::total_cmp);
    levels.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let ordinal = |weight: f64| {
        if levels.len() <= 1 {
            return 1.0;
        }
        let rank = levels.partition_point(|level| *level < weight - 1e-9);
        0.3 + 0.7 * rank as f64 / (levels.len() - 1) as f64
    };
    edges
        .iter()
        .filter(|(a, b, _)| a != b && *a < n && *b < n)
        .map(|(a, b, w)| (*a, *b, ordinal(*w)))
        .collect()
}

/// Two-level force layout of one scene, in footprint units (the scene's
/// area-equivalent radius is 1):
/// - artists: springs on internal edges (weight is ordinal), repulsion
///   between artists closer than two spacings (a sorted grid keeps it
///   linear), so neighbourhoods hold together and nobody overlaps;
/// - knots (the scene's sub-communities): each member is drawn to its
///   knot's centre, and knots repel each other by size, so sub-communities
///   form separate clumps instead of one even disk;
/// - a pull toward the scene centre that grows with distance, so loners
///   settle in the gaps and at the edges rather than drifting off.
///
/// Displacement per step is capped by a cooling temperature. Deterministic:
/// fixed orders everywhere, hashed directions for exact overlaps.
fn simulate_inner(
    pos: &mut [(f64, f64)],
    links: &[(usize, usize, f64)],
    knots: &[Option<u32>],
    spacing: f64,
    iterations: usize,
    t0: f64,
    ids: &[i64],
) {
    let n = pos.len();
    let cutoff = 2.0 * spacing;
    let (k2, cut2) = (spacing * spacing, cutoff * cutoff);
    let knot_count = knots
        .iter()
        .flatten()
        .map(|k| *k as usize + 1)
        .max()
        .unwrap_or(0);
    let mut knot_size = vec![0.0f64; knot_count];
    for k in knots.iter().flatten() {
        knot_size[*k as usize] += 1.0;
    }
    let live: Vec<usize> = (0..knot_count).filter(|k| knot_size[*k] > 0.0).collect();
    let mut centre = vec![(0.0f64, 0.0f64); knot_count];
    let mut push = vec![(0.0f64, 0.0f64); knot_count];
    let mut disp = vec![(0.0f64, 0.0f64); n];
    let mut cells: Vec<(i64, i64, usize)> = Vec::with_capacity(n);
    for step in 0..iterations {
        let t = t0 * (1.0 - step as f64 / iterations as f64) + t0 * 0.02;
        for d in disp.iter_mut() {
            *d = (0.0, 0.0);
        }
        // Artist repulsion, local.
        cells.clear();
        cells.extend((0..n).map(|i| {
            (
                (pos[i].0 / cutoff).floor() as i64,
                (pos[i].1 / cutoff).floor() as i64,
                i,
            )
        }));
        cells.sort_unstable();
        for &(cx, cy, i) in &cells {
            for gx in cx - 1..=cx + 1 {
                let start = cells.partition_point(|c| (c.0, c.1) < (gx, cy - 1));
                for &(ox, oy, j) in &cells[start..] {
                    if ox != gx || oy > cy + 1 {
                        break;
                    }
                    if j == i {
                        continue;
                    }
                    let (dx, dy) = (pos[i].0 - pos[j].0, pos[i].1 - pos[j].1);
                    let d2 = dx * dx + dy * dy;
                    if d2 >= cut2 {
                        continue;
                    }
                    if d2 < 1e-18 {
                        let (angle, _) = unit_pair(ids[i].wrapping_mul(7919) ^ ids[j]);
                        disp[i].0 += spacing * (angle * TAU).cos();
                        disp[i].1 += spacing * (angle * TAU).sin();
                        continue;
                    }
                    let f = k2 / d2;
                    disp[i].0 += dx * f;
                    disp[i].1 += dy * f;
                }
            }
        }
        // Edge springs.
        for &(a, b, w) in links {
            let (dx, dy) = (pos[a].0 - pos[b].0, pos[a].1 - pos[b].1);
            let f = w * (dx * dx + dy * dy).sqrt() / spacing;
            disp[a].0 -= dx * f;
            disp[a].1 -= dy * f;
            disp[b].0 += dx * f;
            disp[b].1 += dy * f;
        }
        // Knots: cohesion toward their centre, repulsion between knots.
        for c in centre.iter_mut() {
            *c = (0.0, 0.0);
        }
        for (i, knot) in knots.iter().enumerate() {
            if let Some(k) = knot {
                let size = knot_size[*k as usize];
                centre[*k as usize].0 += pos[i].0 / size;
                centre[*k as usize].1 += pos[i].1 / size;
            }
        }
        for p in push.iter_mut() {
            *p = (0.0, 0.0);
        }
        for (x, &a) in live.iter().enumerate() {
            for &b in &live[x + 1..] {
                let (mut dx, mut dy) = (centre[a].0 - centre[b].0, centre[a].1 - centre[b].1);
                let mut d2 = dx * dx + dy * dy;
                if d2 < 1e-18 {
                    let (angle, _) = unit_pair((a as i64) * 104_729 + b as i64);
                    dx = spacing * (angle * TAU).cos();
                    dy = spacing * (angle * TAU).sin();
                    d2 = k2;
                }
                let f = KNOT_REPULSION * k2 * knot_size[a] * knot_size[b] / d2;
                push[a].0 += dx * f / knot_size[a];
                push[a].1 += dy * f / knot_size[a];
                push[b].0 -= dx * f / knot_size[b];
                push[b].1 -= dy * f / knot_size[b];
            }
        }
        for (i, knot) in knots.iter().enumerate() {
            if let Some(k) = knot {
                let k = *k as usize;
                disp[i].0 += push[k].0 - (pos[i].0 - centre[k].0) * KNOT_COHESION;
                disp[i].1 += push[k].1 - (pos[i].1 - centre[k].1) * KNOT_COHESION;
            }
        }
        // Gravity, then move.
        for i in 0..n {
            disp[i].0 -= pos[i].0 * INNER_GRAVITY;
            disp[i].1 -= pos[i].1 * INNER_GRAVITY;
            let len = (disp[i].0 * disp[i].0 + disp[i].1 * disp[i].1).sqrt();
            if len > 0.0 {
                let s = len.min(t) / len;
                pos[i].0 += disp[i].0 * s;
                pos[i].1 += disp[i].1 * s;
            }
        }
    }
}

/// The scene's sub-communities (Louvain over its internal edges); members
/// of groups under `MIN_KNOT` are loners (None).
fn knots_of(links: &[(usize, usize, f64)], n: usize) -> Vec<Option<u32>> {
    let edges: Vec<(u32, u32, f64)> = links
        .iter()
        .map(|(a, b, w)| (*a as u32, *b as u32, *w))
        .collect();
    let labels = louvain::louvain(&Csr::from_edges(n, &edges), 1.0);
    let mut size = vec![0usize; n];
    for label in &labels {
        size[*label as usize] += 1;
    }
    labels
        .into_iter()
        .map(|label| (size[label as usize] >= MIN_KNOT).then_some(label))
        .collect()
}

/// Ease a point (footprint units) inside the scene's bound: unchanged up to
/// the knee, then squeezed smoothly so nothing reaches `PACK`.
fn soft_clip(p: (f64, f64)) -> (f64, f64) {
    let rho = (p.0 * p.0 + p.1 * p.1).sqrt();
    let knee = SOFT_KNEE * PACK;
    if rho <= knee {
        return p;
    }
    let room = PACK - knee;
    let eased = knee + room * ((rho - knee) / room).tanh();
    (p.0 * eased / rho, p.1 * eased / rho)
}

/// One scene's own layout, in bound units (the scene's `r` is 1, nothing
/// reaches it). `ids` are the members hubs first; `edges` the scene's
/// internal edges (local indices, provider weight); `prev` where each
/// member sat last build, if it was in this scene.
///
/// Cold (nothing remembered): hubs start at the centre, everyone else
/// outward at a hashed angle; the scene's sub-communities (Louvain over its
/// own edges) become knots and `simulate_inner` pulls each knot together
/// and the knots apart, so the outline follows them; the cloud is centred,
/// scaled so three quarters of it sits within the footprint (denser scenes
/// a little tighter) and its outliers eased in. Warm with few newcomers:
/// everyone remembered stays where they were and newcomers sit by their
/// neighbours, so an unchanged scene comes back bit for bit. Warm with many
/// newcomers (over `RESHAPE_SHARE`): the whole scene relaxes from where it
/// was.
pub fn inner_layout(
    ids: &[i64],
    edges: &[(usize, usize, f64)],
    prev: &[Option<(f64, f64)>],
) -> Vec<(f64, f64)> {
    let n = ids.len();
    if n == 0 {
        return Vec::new();
    }
    let remembered: Vec<Option<(f64, f64)>> =
        (0..n).map(|i| prev.get(i).copied().flatten()).collect();
    let known = remembered.iter().filter(|p| p.is_some()).count();
    let newcomers = n - known;
    let reshape = known > 0 && newcomers as f64 > RESHAPE_SHARE * n as f64;
    let refine = known > 0 && !reshape;
    if refine && newcomers == 0 {
        return remembered.into_iter().flatten().collect();
    }
    if known == 0 && n == 1 {
        return vec![(0.0, 0.0)];
    }
    let links = ordinal_links(edges, n);
    let mut neighbours: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for &(a, b, w) in &links {
        neighbours[a].push((b, w));
        neighbours[b].push((a, w));
    }
    let density = 2.0 * links.iter().map(|l| l.2).sum::<f64>() / n as f64;
    let spacing = (PI / n as f64).sqrt() * INNER_SPACING;

    // Starting points, in footprint units.
    let mut pos = vec![(0.0f64, 0.0f64); n];
    let mut placed = vec![false; n];
    for (i, p) in remembered.iter().enumerate() {
        if let Some((x, y)) = p {
            pos[i] = (x * PACK, y * PACK);
            placed[i] = true;
        }
    }
    for i in 0..n {
        if placed[i] {
            continue;
        }
        let (angle, wobble) = unit_pair(ids[i]);
        let theta = angle * TAU;
        pos[i] = if known == 0 {
            // Cold: by rank, hubs in the middle.
            let rho = 0.8 * ((i as f64 + 0.5) / n as f64).sqrt();
            (rho * theta.cos(), rho * theta.sin())
        } else {
            // A newcomer starts next to the neighbours already down.
            let (mut sx, mut sy, mut total) = (0.0, 0.0, 0.0);
            for &(j, w) in &neighbours[i] {
                if placed[j] {
                    sx += pos[j].0 * w;
                    sy += pos[j].1 * w;
                    total += w;
                }
            }
            if total > 0.0 {
                (
                    sx / total + spacing * theta.cos(),
                    sy / total + spacing * theta.sin(),
                )
            } else {
                let rho = 0.85 * wobble.sqrt();
                (rho * theta.cos(), rho * theta.sin())
            }
        };
        placed[i] = true;
    }

    if refine {
        // Remembered artists stay put; newcomers sit by their neighbours,
        // eased inside the bound.
        for (i, p) in pos.iter_mut().enumerate() {
            if remembered[i].is_none() {
                *p = soft_clip(*p);
            }
        }
    } else {
        // Cold starts far from the layout's own scale, a reshape close to it.
        let t0 = if reshape { 0.03 } else { 0.1 };
        let knots = knots_of(&links, n);
        simulate_inner(&mut pos, &links, &knots, spacing, INNER_ITERATIONS, t0, ids);
        let count = n as f64;
        let (cx, cy) = pos.iter().fold((0.0, 0.0), |acc, p| {
            (acc.0 + p.0 / count, acc.1 + p.1 / count)
        });
        for p in pos.iter_mut() {
            p.0 -= cx;
            p.1 -= cy;
        }
        // Scale by the radius three quarters of the artists sit within, so a
        // few far loners cannot shrink the whole scene.
        let mut radii: Vec<f64> = pos.iter().map(|p| (p.0 * p.0 + p.1 * p.1).sqrt()).collect();
        radii.sort_by(f64::total_cmp);
        let reach = radii[(radii.len() * 3) / 4];
        let target = INNER_REACH_SPARSE
            - (INNER_REACH_SPARSE - INNER_REACH_DENSE) * ((density - 1.0) / 5.0).clamp(0.0, 1.0);
        if reach > 1e-12 {
            let scale = target / reach;
            for p in pos.iter_mut() {
                p.0 *= scale;
                p.1 *= scale;
            }
        }
        for p in pos.iter_mut() {
            *p = soft_clip(*p);
        }
    }
    pos.into_iter().map(|(x, y)| (x / PACK, y / PACK)).collect()
}

/// Ring group of a name: '#' for digits, a-z, '~' for everything else.
fn initial(name: &str) -> char {
    match name_key(name).chars().next() {
        Some(c) if c.is_ascii_digit() => '#',
        Some(c) if c.is_ascii_lowercase() => c,
        _ => '~',
    }
}

/// The uncharted ring: grouped by name initial clockwise from the top, each
/// group's arc sized by its count, a small gap between groups, radius
/// jittered inside the band.
pub fn place_ring(uncharted: &[(i64, String)]) -> Vec<(i64, f64, f64)> {
    if uncharted.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<(char, String, i64)> = uncharted
        .iter()
        .map(|(id, name)| (initial(name), name_key(name), *id))
        .collect();
    sorted.sort();
    let mut groups: Vec<(char, usize)> = Vec::new();
    for (group, _, _) in &sorted {
        match groups.last_mut() {
            Some((last, count)) if last == group => *count += 1,
            _ => groups.push((*group, 1)),
        }
    }
    let gap = if groups.len() > 1 { 0.02 } else { 0.0 };
    let usable = TAU - gap * groups.len() as f64;
    let total = sorted.len() as f64;
    let mut out = Vec::with_capacity(sorted.len());
    let mut start = -PI / 2.0;
    let mut cursor = 0;
    for (_, count) in groups {
        let span = usable * count as f64 / total;
        for k in 0..count {
            let (_, _, id) = &sorted[cursor + k];
            let (_, wobble) = unit_pair(*id);
            let theta = start + span * (k as f64 + 0.5) / count as f64;
            let dist = RING_INNER + RING_BAND * wobble;
            out.push((
                *id,
                CENTRE + dist * theta.cos(),
                CENTRE + dist * theta.sin(),
            ));
        }
        cursor += count;
        start += span + gap;
    }
    out
}

struct SceneRow {
    id: i64,
    videos: i64,
    artists: i64,
    prev: Option<(f64, f64, f64)>,
}

/// What one scene's interior layout needs.
struct Interior {
    ids: Vec<i64>,
    edges: Vec<(usize, usize, f64)>,
    prev: Vec<Option<(f64, f64)>>,
}

/// One row of video_artist_facets as the layout reads it.
struct ArtistRow {
    id: i64,
    name: String,
    scene: Option<i64>,
    videos: i64,
    x: Option<f64>,
    y: Option<f64>,
}

/// The scene's members (ranked, hubs first), its internal edges and where
/// each member sat inside it last build. A remembered position counts only
/// when it lies inside this scene's previous bound.
fn interior(
    ranked: Vec<i64>,
    graph: &ArtistGraph,
    previous_spot: &HashMap<i64, (f64, f64)>,
    scene_prev: Option<(f64, f64, f64)>,
) -> Interior {
    let local: HashMap<i64, usize> = ranked.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut edges = Vec::new();
    for (i, id) in ranked.iter().enumerate() {
        for (neighbour, weight, _) in graph.neighbors(*id) {
            if let Some(&j) = local.get(&neighbour)
                && j > i
            {
                edges.push((i, j, weight));
            }
        }
    }
    let prev = ranked
        .iter()
        .map(|id| {
            let (px, py, pr) = scene_prev?;
            let (x, y) = previous_spot.get(id)?;
            let rel = ((x - px) / pr, (y - py) / pr);
            (rel.0 * rel.0 + rel.1 * rel.1 <= 1.0 + 1e-9).then_some(rel)
        })
        .collect();
    Interior {
        ids: ranked,
        edges,
        prev,
    }
}

/// Rebuild this stage's rows. Returns the number of artists placed.
pub fn build(conn: &Connection) -> Result<usize> {
    use rayon::prelude::*;

    let scenes: Vec<SceneRow> = conn
        .prepare("SELECT scene_id, video_count, artist_count, x, y, r FROM video_scenes ORDER BY scene_id")?
        .query_map([], |row| {
            let r: f64 = row.get(5)?;
            Ok(SceneRow {
                id: row.get(0)?,
                videos: row.get(1)?,
                artists: row.get(2)?,
                prev: (r > 0.0).then_some((row.get(3)?, row.get(4)?, r)),
            })
        })?
        .collect::<Result<_, _>>()?;
    let artists: Vec<ArtistRow> = conn
        .prepare(
            "SELECT artist_tidal_id, name, scene_id, videos, x, y FROM video_artist_facets
              ORDER BY artist_tidal_id",
        )?
        .query_map([], |row| {
            Ok(ArtistRow {
                id: row.get(0)?,
                name: row.get(1)?,
                scene: row.get(2)?,
                videos: row.get(3)?,
                x: row.get(4)?,
                y: row.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let graph = scenes::cached_graph(conn)?;
    let popularity: HashMap<i64, i64> = conn
        .prepare("SELECT artist_tidal_id, popularity FROM video_artist_state WHERE popularity IS NOT NULL")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    // Interiors from another layout version are redrawn cold, once.
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [LAYOUT_VERSION_KEY],
            |row| row.get(0),
        )
        .optional()?;
    let remember_interiors = version.as_deref() == Some(LAYOUT_VERSION);

    let index_of_scene: HashMap<i64, usize> =
        scenes.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    let scene_index_of: HashMap<i64, usize> = artists
        .iter()
        .filter_map(|a| {
            a.scene
                .and_then(|s| index_of_scene.get(&s))
                .map(|i| (a.id, *i))
        })
        .collect();
    let artists_per_scene: Vec<i64> = scenes.iter().map(|s| s.artists).collect();
    let links = scene_links(&graph, &scene_index_of, &artists_per_scene);
    let nodes: Vec<SceneNode> = scenes
        .iter()
        .map(|s| SceneNode {
            id: s.id,
            videos: s.videos,
            prev: s.prev,
        })
        .collect();
    let placements = place_scenes(&nodes, &links);

    let previous_spot: HashMap<i64, (f64, f64)> = if remember_interiors {
        artists
            .iter()
            .filter_map(|a| Some((a.id, (a.x?, a.y?))))
            .collect()
    } else {
        HashMap::new()
    };
    let mut members: Vec<Vec<(f64, i64)>> = vec![Vec::new(); scenes.len()];
    let mut uncharted = Vec::new();
    for artist in &artists {
        match scene_index_of.get(&artist.id) {
            Some(&scene) => members[scene].push((
                scenes::exemplar_score(
                    artist.videos,
                    graph.degree(artist.id),
                    popularity.get(&artist.id).copied(),
                ),
                artist.id,
            )),
            None => uncharted.push((artist.id, artist.name.clone())),
        }
    }
    let interiors: Vec<Interior> = members
        .into_iter()
        .enumerate()
        .map(|(scene, mut list)| {
            list.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            let ranked = list.into_iter().map(|(_, id)| id).collect();
            interior(ranked, &graph, &previous_spot, scenes[scene].prev)
        })
        .collect();
    // Scenes are independent: lay them out in parallel, collect in order.
    let shapes: Vec<Vec<(f64, f64)>> = interiors
        .par_iter()
        .map(|inner| inner_layout(&inner.ids, &inner.edges, &inner.prev))
        .collect();

    let mut spots = Vec::with_capacity(artists.len());
    for ((inner, shape), place) in interiors.iter().zip(&shapes).zip(&placements) {
        for (id, (x, y)) in inner.ids.iter().zip(shape) {
            spots.push((*id, place.x + x * place.r, place.y + y * place.r));
        }
    }
    spots.extend(place_ring(&uncharted));

    let tx = conn.unchecked_transaction()?;
    {
        let mut update = tx.prepare(
            "UPDATE video_scenes SET x = ?2, y = ?3, r = ?4, hue = ?5 WHERE scene_id = ?1",
        )?;
        for (scene, place) in scenes.iter().zip(&placements) {
            update.execute(params![scene.id, place.x, place.y, place.r, place.hue])?;
        }
        let mut update =
            tx.prepare("UPDATE video_artist_facets SET x = ?2, y = ?3 WHERE artist_tidal_id = ?1")?;
        for (id, x, y) in &spots {
            update.execute(params![id, x, y])?;
        }
        tx.execute(
            "INSERT INTO server_config (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![LAYOUT_VERSION_KEY, LAYOUT_VERSION],
        )?;
    }
    tx.commit()?;
    Ok(spots.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i64, videos: i64) -> SceneNode {
        SceneNode {
            id,
            videos,
            prev: None,
        }
    }

    fn no_overlap(places: &[Placement]) -> bool {
        for (i, a) in places.iter().enumerate() {
            for b in &places[i + 1..] {
                let d = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
                if d + 1e-6 < a.r + b.r {
                    return false;
                }
            }
        }
        true
    }

    fn sample() -> (Vec<SceneNode>, Vec<SceneLink>) {
        let nodes: Vec<SceneNode> = (1..=12).map(|i| node(i, 300 + i * 250)).collect();
        let links = vec![
            SceneLink {
                a: 0,
                b: 1,
                weight: 1.0,
            },
            SceneLink {
                a: 1,
                b: 2,
                weight: 0.6,
            },
            SceneLink {
                a: 3,
                b: 4,
                weight: 0.9,
            },
            SceneLink {
                a: 5,
                b: 6,
                weight: 0.4,
            },
            SceneLink {
                a: 0,
                b: 7,
                weight: 0.2,
            },
            SceneLink {
                a: 8,
                b: 9,
                weight: 0.7,
            },
        ];
        (nodes, links)
    }

    #[test]
    fn a_cold_layout_is_deterministic_inside_the_field_and_overlap_free() {
        let (nodes, links) = sample();
        let first = place_scenes(&nodes, &links);
        assert_eq!(first, place_scenes(&nodes, &links));
        assert!(no_overlap(&first));
        for place in &first {
            let reach = ((place.x - CENTRE).powi(2) + (place.y - CENTRE).powi(2)).sqrt() + place.r;
            assert!(reach <= SCENE_FIELD + 1e-6, "{reach}");
            assert!((0.0..360.0).contains(&place.hue));
        }
        // Radius follows sqrt(videos).
        let ratio = first[11].r / first[0].r;
        assert!((ratio - (3300.0f64 / 550.0).sqrt()).abs() < 1e-9);
        // Linked scenes sit closer than unlinked ones on average.
        let dist = |a: usize, b: usize| {
            ((first[a].x - first[b].x).powi(2) + (first[a].y - first[b].y).powi(2)).sqrt()
        };
        assert!(dist(0, 1) < dist(0, 10));
    }

    #[test]
    fn a_warm_start_reproduces_the_previous_map() {
        let (mut nodes, links) = sample();
        let first = place_scenes(&nodes, &links);
        for (node, place) in nodes.iter_mut().zip(&first) {
            node.prev = Some((place.x, place.y, place.r));
        }
        let second = place_scenes(&nodes, &links);
        for (a, b) in first.iter().zip(&second) {
            assert!((a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6);
            assert!((a.hue - b.hue).abs() < 1e-6);
        }
        // A new scene joins next to the one it links to; the others barely move.
        nodes.push(node(13, 900));
        let mut links = links;
        links.push(SceneLink {
            a: 3,
            b: 12,
            weight: 1.0,
        });
        let third = place_scenes(&nodes, &links);
        assert!(no_overlap(&third));
        let moved = first
            .iter()
            .zip(&third)
            .map(|(a, b)| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt())
            .fold(0.0, f64::max);
        assert!(moved < 60.0, "kept scenes moved {moved}");
    }

    /// Two knots of ten (0..10 and 10..20) joined by one weak edge, plus
    /// five loners (20..25).
    fn two_knots() -> (Vec<i64>, Vec<(usize, usize, f64)>) {
        let ids: Vec<i64> = (0..25).map(|i| 1000 + i).collect();
        let mut edges = Vec::new();
        for start in [0usize, 10] {
            for a in start..start + 10 {
                for b in a + 1..start + 10 {
                    edges.push((a, b, if (a + b) % 3 == 0 { 0.9 } else { 0.8 }));
                }
            }
        }
        edges.push((9, 10, 0.3));
        (ids, edges)
    }

    fn mean_distance(points: &[(f64, f64)], a: &[usize], b: &[usize]) -> f64 {
        let mut total = 0.0;
        let mut count = 0.0;
        for &i in a {
            for &j in b {
                if i != j {
                    total += ((points[i].0 - points[j].0).powi(2)
                        + (points[i].1 - points[j].1).powi(2))
                    .sqrt();
                    count += 1.0;
                }
            }
        }
        total / count
    }

    #[test]
    fn a_scene_interior_is_deterministic_bounded_and_knotted() {
        let (ids, edges) = two_knots();
        let none = vec![None; ids.len()];
        let first = inner_layout(&ids, &edges, &none);
        assert_eq!(first, inner_layout(&ids, &edges, &none));
        assert!(first.iter().all(|p| p.0 * p.0 + p.1 * p.1 < 1.0));
        let a: Vec<usize> = (0..10).collect();
        let b: Vec<usize> = (10..20).collect();
        let within = (mean_distance(&first, &a, &a) + mean_distance(&first, &b, &b)) / 2.0;
        let across = mean_distance(&first, &a, &b);
        assert!(within < 0.6 * across, "knots: {within} vs {across}");
        // A lone artist sits at the centre; an empty scene has no shape.
        assert_eq!(inner_layout(&ids[..1], &[], &[None]), vec![(0.0, 0.0)]);
        assert!(inner_layout(&[], &[], &[]).is_empty());
    }

    #[test]
    fn a_remembered_scene_keeps_its_shape_and_newcomers_settle_near_friends() {
        let (mut ids, mut edges) = two_knots();
        let none = vec![None; ids.len()];
        let first = inner_layout(&ids, &edges, &none);
        let remembered: Vec<Option<(f64, f64)>> = first.iter().copied().map(Some).collect();
        assert_eq!(
            inner_layout(&ids, &edges, &remembered),
            first,
            "bit for bit"
        );
        // One newcomer tied to the first knot.
        ids.push(2000);
        for friend in 0..4 {
            edges.push((friend, 25, 0.9));
        }
        let mut with_newcomer = remembered.clone();
        with_newcomer.push(None);
        let grown = inner_layout(&ids, &edges, &with_newcomer);
        assert_eq!(&grown[..25], &first[..], "remembered artists stay put");
        let newcomer = grown[25];
        assert!(newcomer.0 * newcomer.0 + newcomer.1 * newcomer.1 < 1.0);
        let to_a = mean_distance(&grown, &[25], &(0..10).collect::<Vec<_>>());
        let to_b = mean_distance(&grown, &[25], &(10..20).collect::<Vec<_>>());
        assert!(to_a < to_b);
        // Many newcomers: the scene relaxes as a whole, still bounded and
        // deterministic.
        let mostly_new: Vec<Option<(f64, f64)>> =
            (0..ids.len()).map(|i| (i < 8).then(|| first[i])).collect();
        let reshaped = inner_layout(&ids, &edges, &mostly_new);
        assert_eq!(reshaped, inner_layout(&ids, &edges, &mostly_new));
        assert!(reshaped.iter().all(|p| p.0 * p.0 + p.1 * p.1 < 1.0));
    }

    #[test]
    fn denser_scenes_draw_tighter_clouds() {
        let ids: Vec<i64> = (0..40).collect();
        let sparse: Vec<(usize, usize, f64)> = (0..39).map(|i| (i, i + 1, 0.8)).collect();
        let mut dense = sparse.clone();
        for a in 0..40 {
            for step in [2, 3, 5, 7] {
                dense.push((a, (a + step) % 40, 0.8));
            }
        }
        let spread = |points: &[(f64, f64)]| {
            (points.iter().map(|p| p.0 * p.0 + p.1 * p.1).sum::<f64>() / points.len() as f64).sqrt()
        };
        let none = vec![None; 40];
        assert!(
            spread(&inner_layout(&ids, &dense, &none))
                < spread(&inner_layout(&ids, &sparse, &none))
        );
    }

    #[test]
    fn the_ring_groups_by_initial_clockwise_from_the_top() {
        let names = [
            (1, "Abba"),
            (2, "beck"),
            (3, "The Beatles"),
            (4, "2Pac"),
            (5, "\u{30a2}"),
        ];
        let input: Vec<(i64, String)> = names.iter().map(|(id, n)| (*id, n.to_string())).collect();
        let spots = place_ring(&input);
        assert_eq!(spots.len(), 5);
        let angle = |id: i64| {
            let s = spots.iter().find(|s| s.0 == id).unwrap();
            ((s.2 - CENTRE).atan2(s.1 - CENTRE) + PI / 2.0).rem_euclid(TAU)
        };
        assert!(angle(4) < angle(1), "digits first");
        assert!(angle(1) < angle(3));
        assert!(
            angle(3) < angle(2),
            "a leading 'the' is ignored: beatles before beck"
        );
        assert!(angle(2) < angle(5), "other scripts last");
        for s in &spots {
            let d = ((s.1 - CENTRE).powi(2) + (s.2 - CENTRE).powi(2)).sqrt();
            assert!((RING_INNER..=RING_INNER + RING_BAND).contains(&d));
        }
    }

    #[test]
    fn build_places_every_artist_and_hues_follow_angle() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO video_scenes (scene_id, name, video_count, artist_count) VALUES (1, 'A', 900, 2), (2, 'B', 400, 1);
             INSERT INTO video_artist_facets (artist_tidal_id, name, scene_id, videos, charted) VALUES
                 (10, 'Ten', 1, 500, 1), (11, 'Eleven', 1, 400, 1), (12, 'Twelve', 2, 400, 1), (13, 'Zed', NULL, 3, 0);
             INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, weight)
                 VALUES (10, 11, 'x', 'lastfm', 0.9), (11, 12, 'x', 'lastfm', 0.5);",
        )
        .unwrap();
        assert_eq!(build(&conn).unwrap(), 4);
        let first: Vec<(f64, f64, f64)> = conn
            .prepare("SELECT x, y, hue FROM video_scenes ORDER BY scene_id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for (x, y, hue) in &first {
            assert!((hue_of(*x, *y) - hue).abs() < 1e-9);
        }
        assert_eq!(build(&conn).unwrap(), 4);
        let second: Vec<(f64, f64, f64)> = conn
            .prepare("SELECT x, y, hue FROM video_scenes ORDER BY scene_id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for (a, b) in first.iter().zip(&second) {
            assert!((a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6);
        }
        // Every charted artist inside its own scene's bound; bounds apart.
        let scenes: Vec<(i64, f64, f64, f64)> = conn
            .prepare("SELECT scene_id, x, y, r FROM video_scenes ORDER BY scene_id")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let spots = |conn: &Connection| -> Vec<(i64, i64, f64, f64)> {
            conn.prepare(
                "SELECT artist_tidal_id, scene_id, x, y FROM video_artist_facets
                  WHERE scene_id IS NOT NULL ORDER BY artist_tidal_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
        };
        let placed = spots(&conn);
        for (_, scene, x, y) in &placed {
            let (_, cx, cy, r) = scenes.iter().find(|s| s.0 == *scene).unwrap();
            assert!(((x - cx).powi(2) + (y - cy).powi(2)).sqrt() < *r);
        }
        let (a, b) = (scenes[0], scenes[1]);
        assert!(((a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt() >= a.3 + b.3);
        // A third build over the same data moves no artist at all.
        build(&conn).unwrap();
        for (before, after) in placed.iter().zip(spots(&conn)) {
            assert!((before.2 - after.2).abs() < 1e-9 && (before.3 - after.3).abs() < 1e-9);
        }
        let version: String = conn
            .query_row(
                "SELECT value FROM server_config WHERE key = ?1",
                [LAYOUT_VERSION_KEY],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, LAYOUT_VERSION);
        let ring: (f64, f64) = conn
            .query_row(
                "SELECT x, y FROM video_artist_facets WHERE artist_tidal_id = 13",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let d = ((ring.0 - CENTRE).powi(2) + (ring.1 - CENTRE).powi(2)).sqrt();
        assert!(d >= RING_INNER);
    }
}
