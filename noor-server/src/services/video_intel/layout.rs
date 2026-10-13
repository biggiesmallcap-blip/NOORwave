//! Map layout: scene disks from a force layout of the scene graph, artists
//! inside their disk by rank and hashed angle, uncharted artists on an outer
//! ring, warm-started so nothing jumps between builds.
//!
//! The map is a 1000 x 1000 square, origin top-left. Scene disks live inside
//! a circle of radius 400 around the centre; the uncharted ring runs from
//! 425 to 470. A disk's radius is proportional to sqrt(videos). Hue is the
//! angle of a scene's centre around the map centre, so color is place.
//!
//! Warm start: when most scenes continue from the last build, those keep
//! their positions and only new scenes are simulated; a final overlap pass
//! moves anyone just enough to make room for growth. On unchanged data a
//! rebuild reproduces the previous map exactly.

use std::collections::{BTreeMap, HashMap};
use std::f64::consts::{PI, TAU};

use anyhow::Result;
use rusqlite::{Connection, params};

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
/// Fraction of a disk's radius kept free at its centre, and the band an
/// artist's radius jitters within.
const CORE: f64 = 0.10;
const SPREAD: f64 = 0.86;

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

/// Place scene disks. Pure and deterministic.
pub fn place_scenes(nodes: &[SceneNode], links: &[SceneLink]) -> Vec<Placement> {
    let n = nodes.len();
    if n == 0 {
        return Vec::new();
    }
    let ids: Vec<i64> = nodes.iter().map(|node| node.id).collect();
    let radius: Vec<f64> = nodes
        .iter()
        .map(|node| (node.videos.max(1) as f64).sqrt())
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
    // Biggest first on a golden-angle spiral: a compact, deterministic start.
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
            for link in links {
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
    } else {
        for (slot, &i) in order.iter().enumerate() {
            pos[i] = spiral(slot);
        }
    }

    let iterations = if warm {
        WARM_ITERATIONS
    } else {
        COLD_ITERATIONS
    };
    let any_movable = movable.iter().any(|m| *m);
    if any_movable {
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
                    push_apart(&mut pos, &radius, &movable, i, j, &ids);
                }
            }
        }
    }
    // Settle: everyone may move now, just enough to clear overlaps.
    let everyone = vec![true; n];
    for _ in 0..SETTLE_PASSES {
        let mut moved = false;
        for i in 0..n {
            for j in i + 1..n {
                moved |= push_apart(&mut pos, &radius, &everyone, i, j, &ids);
            }
        }
        if !moved {
            break;
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

/// Members inside their disk: rank 0 (the hub) near the centre, the last
/// at the rim, each at its own hashed angle.
pub fn place_members(scene: &Placement, ranked: &[i64]) -> Vec<(i64, f64, f64)> {
    let n = ranked.len().max(1) as f64;
    ranked
        .iter()
        .enumerate()
        .map(|(rank, id)| {
            let (angle, wobble) = unit_pair(*id);
            let fraction = ((rank as f64 + 0.5) / n).sqrt();
            let dist = scene.r * (CORE + SPREAD * fraction) * (0.94 + 0.06 * wobble);
            let theta = angle * TAU;
            (
                *id,
                scene.x + dist * theta.cos(),
                scene.y + dist * theta.sin(),
            )
        })
        .collect()
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

/// Rebuild this stage's rows. Returns the number of artists placed.
pub fn build(conn: &Connection) -> Result<usize> {
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
    let artists: Vec<(i64, String, Option<i64>, i64)> = conn
        .prepare("SELECT artist_tidal_id, name, scene_id, videos FROM video_artist_facets ORDER BY artist_tidal_id")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
        .collect::<Result<_, _>>()?;
    let graph = scenes::cached_graph(conn)?;
    let popularity: HashMap<i64, i64> = conn
        .prepare("SELECT artist_tidal_id, popularity FROM video_artist_state WHERE popularity IS NOT NULL")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;

    let index_of_scene: HashMap<i64, usize> =
        scenes.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    let scene_index_of: HashMap<i64, usize> = artists
        .iter()
        .filter_map(|(id, _, scene, _)| {
            scene
                .and_then(|s| index_of_scene.get(&s))
                .map(|i| (*id, *i))
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

    let mut members: Vec<Vec<(f64, i64)>> = vec![Vec::new(); scenes.len()];
    let mut uncharted = Vec::new();
    for (id, name, _, videos) in &artists {
        match scene_index_of.get(id) {
            Some(&scene) => members[scene].push((
                scenes::exemplar_score(*videos, graph.degree(*id), popularity.get(id).copied()),
                *id,
            )),
            None => uncharted.push((*id, name.clone())),
        }
    }
    let mut spots = Vec::with_capacity(artists.len());
    for (scene, list) in members.iter_mut().enumerate() {
        list.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let ranked: Vec<i64> = list.iter().map(|(_, id)| *id).collect();
        spots.extend(place_members(&placements[scene], &ranked));
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

    #[test]
    fn members_fill_their_disk_hubs_inward() {
        let scene = Placement {
            x: 300.0,
            y: 300.0,
            r: 50.0,
            hue: 0.0,
        };
        let ranked: Vec<i64> = (1..=200).collect();
        let spots = place_members(&scene, &ranked);
        assert_eq!(spots, place_members(&scene, &ranked));
        let dist = |s: &(i64, f64, f64)| ((s.1 - 300.0).powi(2) + (s.2 - 300.0).powi(2)).sqrt();
        assert!(spots.iter().all(|s| dist(s) <= 50.0 + 1e-9));
        assert!(dist(&spots[0]) < 10.0);
        assert!(dist(&spots[199]) > 40.0);
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
