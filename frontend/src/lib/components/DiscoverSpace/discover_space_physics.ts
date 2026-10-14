// Layout and motion for the DiscoverSpace visualization.
// Pure functions on plain arrays - no Svelte reactivity inside.
//
// Position is data, not decoration:
//   - distance from the seed is rank (closer = ranked higher), the same order
//     as the results list;
//   - the angle groups tracks by genre family, one sector per family, sized by
//     how many tracks it holds (tracks with no genre share a last sector).
// The simulation only eases each node toward that target and keeps nodes from
// overlapping. Edges do not pull, so a dense cluster of links cannot drag a
// weak match inward.

import type { DiscoverTrackNode, DiscoverEdge } from './discover_space_types';
import { nodeFamily } from './discover_space_genre';

// Rank 0 (best) sits on the inner radius, the last rank on the outer one.
export const R_NEAR = 150;
export const R_FAR = 520;

/** Rings drawn at the rank quartiles, so the map can be read like the list. */
export const RANK_RINGS = [0.25, 0.5, 0.75].map((q) => ({
	q,
	r: R_NEAR + q * (R_FAR - R_NEAR),
	label: `Top ${Math.round(q * 100)}%`,
}));

const SPRING = 0.06;        // pull toward the target point, per tick
const DAMPING = 0.78;
const MAX_SPEED = 6;        // world units per tick
const COLLIDE_PAD = 6;      // world units kept clear between node edges
const SECTOR_GAP = 0.06;    // radians between family sectors
const GOLDEN = 0.6180339887;

// Below this many nodes collisions are checked pairwise; above, via a grid.
const DIRECT_COLLISION_THRESHOLD = 160;

/** The score the list sorts by: session rerank, then shaped, then normalized. */
export function displayScore(node: DiscoverTrackNode): number {
	return node.rerankScore ?? node.shapedScore ?? node.score;
}

/**
 * Give every non-seed node its rank, target radius and target angle. With
 * `place`, nodes that have never been placed jump straight to their target
 * (a new space appears laid out, instead of flying in from server hints).
 */
export function assignLayoutTargets(nodes: DiscoverTrackNode[], place: boolean): void {
	const stars = nodes.filter((n) => !n.isSeed);
	if (stars.length === 0) return;

	const ranked = stars.toSorted((a, b) => displayScore(b) - displayScore(a) || a.trackId - b.trackId);
	const last = Math.max(1, ranked.length - 1);
	ranked.forEach((node, i) => {
		node.rank = ranked.length > 1 ? i / last : 0;
		node.targetR = R_NEAR + node.rank * (R_FAR - R_NEAR);
	});

	// One sector per genre family, largest first; untagged tracks go last.
	const groups = new Map<string, DiscoverTrackNode[]>();
	for (const node of ranked) {
		const key = nodeFamily(node)?.key ?? '';
		const group = groups.get(key);
		if (group) group.push(node);
		else groups.set(key, [node]);
	}
	const order = [...groups.entries()].sort(([ka, a], [kb, b]) => {
		if (ka === '' || kb === '') return ka === '' ? 1 : -1;
		return b.length - a.length || ka.localeCompare(kb);
	});
	const gaps = order.length > 1 ? order.length * SECTOR_GAP : 0;
	const usable = Math.PI * 2 - gaps;
	let cursor = -Math.PI / 2;
	for (const [, members] of order) {
		const width = (usable * members.length) / ranked.length;
		members.forEach((node, j) => {
			// Golden-ratio spread inside the sector, so neighbours in rank are
			// not also neighbours in angle (fewer collisions on one ring).
			const f = ((j + 0.5) * GOLDEN) % 1;
			node.targetAngle = cursor + width * f;
		});
		cursor += width + (order.length > 1 ? SECTOR_GAP : 0);
	}

	if (place) {
		for (const node of stars) {
			if (node.placed) continue;
			const [tx, ty] = targetPoint(node);
			// Tiny deterministic offset so coincident targets can separate.
			node.x = tx + ((node.trackId % 7) - 3) * 0.5;
			node.y = ty + ((node.trackId % 5) - 2) * 0.5;
			node.vx = 0;
			node.vy = 0;
			node.placed = true;
		}
	}
}

function targetPoint(node: DiscoverTrackNode): [number, number] {
	const r = node.targetR ?? R_FAR;
	const a = node.targetAngle ?? 0;
	return [Math.cos(a) * r, Math.sin(a) * r];
}

// --- Spatial grid -------------------------------------------------------------

interface SpatialCell { nodes: DiscoverTrackNode[]; }

// Cached grid for hit-testing. The grid is rebuilt whenever physics ticks
// (positions move) and reused across pointermove events while settled.
let _cachedGrid: Map<string, SpatialCell> | null = null;
let _cachedGridNodes: DiscoverTrackNode[] | null = null;
let _cachedGridCellSize = 0;

function buildSpatialGrid(nodes: DiscoverTrackNode[], cellSize: number): Map<string, SpatialCell> {
	const grid = new Map<string, SpatialCell>();
	for (const node of nodes) {
		const cx = Math.floor(node.x / cellSize);
		const cy = Math.floor(node.y / cellSize);
		const key = `${cx},${cy}`;
		let cell = grid.get(key);
		if (!cell) { cell = { nodes: [] }; grid.set(key, cell); }
		cell.nodes.push(node);
	}
	return grid;
}

function getOrBuildGrid(nodes: DiscoverTrackNode[], cellSize: number): Map<string, SpatialCell> {
	if (_cachedGrid && _cachedGridNodes === nodes && _cachedGridCellSize === cellSize) {
		return _cachedGrid;
	}
	_cachedGrid = buildSpatialGrid(nodes, cellSize);
	_cachedGridNodes = nodes;
	_cachedGridCellSize = cellSize;
	return _cachedGrid;
}

function invalidateGridCache(): void {
	_cachedGrid = null;
	_cachedGridNodes = null;
}

// --- Collision ----------------------------------------------------------------

function separate(a: DiscoverTrackNode, b: DiscoverTrackNode): void {
	const dx = b.x - a.x;
	const dy = b.y - a.y;
	const min = a.radius + b.radius + COLLIDE_PAD;
	const d2 = dx * dx + dy * dy;
	if (d2 >= min * min) return;
	const dist = Math.sqrt(d2) || 0.01;
	const push = (min - dist) * 0.5;
	const ux = dx / dist;
	const uy = dy / dist;
	// The seed is pinned; the other node takes the whole correction.
	const wa = a.isSeed ? 0 : b.isSeed ? 1 : 0.5;
	const wb = b.isSeed ? 0 : a.isSeed ? 1 : 0.5;
	a.vx -= ux * push * wa;
	a.vy -= uy * push * wa;
	b.vx += ux * push * wb;
	b.vy += uy * push * wb;
}

function resolveCollisions(nodes: DiscoverTrackNode[]): void {
	if (nodes.length <= DIRECT_COLLISION_THRESHOLD) {
		for (let i = 0; i < nodes.length; i++) {
			for (let j = i + 1; j < nodes.length; j++) separate(nodes[i]!, nodes[j]!);
		}
		return;
	}
	const cell = 64;
	const grid = buildSpatialGrid(nodes, cell);
	for (const node of nodes) {
		const cx = Math.floor(node.x / cell);
		const cy = Math.floor(node.y / cell);
		for (let dx = -1; dx <= 1; dx++) {
			for (let dy = -1; dy <= 1; dy++) {
				const bucket = grid.get(`${cx + dx},${cy + dy}`);
				if (!bucket) continue;
				for (const other of bucket.nodes) {
					// Each pair once: order by track id.
					if (other.trackId > node.trackId) separate(node, other);
				}
			}
		}
	}
}

// --- Kinetic energy (for settling) -------------------------------------------

export function kineticEnergy(nodes: DiscoverTrackNode[]): number {
	let e = 0, count = 0;
	for (const n of nodes) {
		if (n.isSeed) continue;
		e += n.vx * n.vx + n.vy * n.vy;
		count++;
	}
	return e / Math.max(1, count);
}

// --- Main force function ------------------------------------------------------

interface PhysicsConfig {
	genreLensActive: boolean;
	prefersReducedMotion: boolean;
}

export function applyForces(
	nodes: DiscoverTrackNode[],
	_edges: DiscoverEdge[],
	config: PhysicsConfig
): void {
	if (nodes.length === 0) return;

	// Ease toward the rank and family target.
	for (const node of nodes) {
		if (node.isSeed) continue;
		const [tx, ty] = targetPoint(node);
		node.vx += (tx - node.x) * SPRING;
		node.vy += (ty - node.y) * SPRING;
	}

	resolveCollisions(nodes);

	// Integrate: pin seeds, dampen + clamp the rest.
	for (const node of nodes) {
		if (node.isSeed) {
			node.x = node.layoutHint?.x ?? 0;
			node.y = node.layoutHint?.y ?? 0;
			node.vx = 0; node.vy = 0;
			continue;
		}
		if (config.prefersReducedMotion) {
			// Jump to the target; collisions still apply on the next tick.
			const [tx, ty] = targetPoint(node);
			node.x += (tx - node.x) * 0.5 + node.vx * 0.5;
			node.y += (ty - node.y) * 0.5 + node.vy * 0.5;
			node.vx = 0; node.vy = 0;
			continue;
		}
		node.vx *= DAMPING;
		node.vy *= DAMPING;
		const speed = Math.sqrt(node.vx * node.vx + node.vy * node.vy);
		if (speed > MAX_SPEED) {
			node.vx = (node.vx / speed) * MAX_SPEED;
			node.vy = (node.vy / speed) * MAX_SPEED;
		}
		node.x += node.vx;
		node.y += node.vy;
	}

	// Positions moved -> invalidate hit-test grid (rebuilt lazily on next use).
	invalidateGridCache();
}

// --- Hover hit-testing --------------------------------------------------------

function findNodeLinear(
	nodes: DiscoverTrackNode[],
	worldX: number,
	worldY: number,
	hitRadius: number
): DiscoverTrackNode | null {
	let best: DiscoverTrackNode | null = null;
	let bestDist = Infinity;
	for (const node of nodes) {
		const dx = node.x - worldX;
		const dy = node.y - worldY;
		const dist = Math.sqrt(dx * dx + dy * dy);
		const threshold = Math.max(node.radius, hitRadius);
		if (dist < threshold && dist < bestDist) { best = node; bestDist = dist; }
	}
	return best;
}

export function findNodeNear(
	nodes: DiscoverTrackNode[],
	worldX: number,
	worldY: number,
	hitRadius = 32
): DiscoverTrackNode | null {
	if (nodes.length < 80) {
		return findNodeLinear(nodes, worldX, worldY, hitRadius);
	}
	const cellSize = 80;
	const grid = getOrBuildGrid(nodes, cellSize);
	const cx = Math.floor(worldX / cellSize);
	const cy = Math.floor(worldY / cellSize);
	let best: DiscoverTrackNode | null = null;
	let bestDist = Infinity;
	for (let dx = -2; dx <= 2; dx++) {
		for (let dy = -2; dy <= 2; dy++) {
			const cell = grid.get(`${cx + dx},${cy + dy}`);
			if (!cell) continue;
			for (const node of cell.nodes) {
				const ddx = node.x - worldX;
				const ddy = node.y - worldY;
				const dist = Math.sqrt(ddx * ddx + ddy * ddy);
				const threshold = Math.max(node.radius, hitRadius);
				if (dist < threshold && dist < bestDist) { best = node; bestDist = dist; }
			}
		}
	}
	return best;
}
