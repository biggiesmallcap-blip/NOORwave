// Force simulation for the DiscoverSpace visualization.
// Pure functions on plain arrays — no Svelte reactivity inside.

import type { DiscoverTrackNode, DiscoverEdge } from './discover_space_types';

// Force constants — tuned for calm, underwater feel
const REPULSION              = 1600;  // node-node push
const ATTRACTION             = 0.002; // edge spring pull
const ANCHOR_GRAVITY         = 0.0002; // pull toward seed — low so nodes orbit at 200–400px, not 76px
const GENRE_CENTROID_STRENGTH = 0.005;

const DAMPING   = 0.82;   // heavy damping — damps 86% velocity in 10 ticks
const MAX_SPEED = 1.2;    // world-units/tick cap — prevents layout-update lurches

// Switch between pairwise direct repulsion and grid-bucketed repulsion.
const DIRECT_REPULSION_THRESHOLD = 80;

// ─── Spatial grid ─────────────────────────────────────────────────────────────

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

function applyGridRepulsion(nodes: DiscoverTrackNode[], grid: Map<string, SpatialCell>, cellSize: number): void {
	for (const node of nodes) {
		const cx = Math.floor(node.x / cellSize);
		const cy = Math.floor(node.y / cellSize);
		for (let dx = -1; dx <= 1; dx++) {
			for (let dy = -1; dy <= 1; dy++) {
				const cell = grid.get(`${cx + dx},${cy + dy}`);
				if (!cell) continue;
				for (const other of cell.nodes) {
					if (other === node) continue;
					const ddx = other.x - node.x;
					const ddy = other.y - node.y;
					const dist = Math.sqrt(ddx * ddx + ddy * ddy) || 1;
					const force = REPULSION / (dist * dist);
					// Asymmetric per-pair (visited twice — once from each end), so
					// the net force matches the symmetric direct path below.
					node.vx -= (ddx / dist) * force;
					node.vy -= (ddy / dist) * force;
				}
			}
		}
	}
}

function applyDirectRepulsion(nodes: DiscoverTrackNode[]): void {
	for (let i = 0; i < nodes.length; i++) {
		for (let j = i + 1; j < nodes.length; j++) {
			const dx = nodes[j]!.x - nodes[i]!.x;
			const dy = nodes[j]!.y - nodes[i]!.y;
			const dist = Math.sqrt(dx * dx + dy * dy) || 1;
			const force = REPULSION / (dist * dist);
			const fx = (dx / dist) * force;
			const fy = (dy / dist) * force;
			nodes[i]!.vx -= fx;
			nodes[i]!.vy -= fy;
			nodes[j]!.vx += fx;
			nodes[j]!.vy += fy;
		}
	}
}

// ─── Genre centroids ──────────────────────────────────────────────────────────

function computeGenreCentroids(nodes: DiscoverTrackNode[]): Map<string, { x: number; y: number }> {
	const sums = new Map<string, { x: number; y: number; count: number }>();
	for (const node of nodes) {
		const key = node.genres[0] ?? node.topGenre ?? 'unknown';
		const entry = sums.get(key) ?? { x: 0, y: 0, count: 0 };
		entry.x += node.x; entry.y += node.y; entry.count += 1;
		sums.set(key, entry);
	}
	const out = new Map<string, { x: number; y: number }>();
	for (const [key, { x, y, count }] of sums) out.set(key, { x: x / count, y: y / count });
	return out;
}

// ─── Kinetic energy (for settling) ───────────────────────────────────────────

export function kineticEnergy(nodes: DiscoverTrackNode[]): number {
	let e = 0, count = 0;
	for (const n of nodes) {
		if (n.isSeed) continue;
		e += n.vx * n.vx + n.vy * n.vy;
		count++;
	}
	return e / Math.max(1, count);
}

// ─── Main force function ──────────────────────────────────────────────────────

interface PhysicsConfig {
	genreLensActive: boolean;
	prefersReducedMotion: boolean;
}

export function applyForces(
	nodes: DiscoverTrackNode[],
	edges: DiscoverEdge[],
	config: PhysicsConfig
): void {
	if (nodes.length === 0) return;

	// ── Repulsion ──────────────────────────────────────────────────────────────
	if (nodes.length > DIRECT_REPULSION_THRESHOLD) {
		const grid = getOrBuildGrid(nodes, 120);
		applyGridRepulsion(nodes, grid, 120);
	} else {
		applyDirectRepulsion(nodes);
	}

	// ── Edge attraction ────────────────────────────────────────────────────────
	const nodeById = new Map(nodes.map((n) => [n.trackId, n]));
	for (const edge of edges) {
		const from = nodeById.get(edge.fromTrackId);
		const to   = nodeById.get(edge.toTrackId);
		if (!from || !to) continue;
		const dx = to.x - from.x;
		const dy = to.y - from.y;
		const dist = Math.sqrt(dx * dx + dy * dy) || 1;
		const force = dist * ATTRACTION * edge.weight;
		const fx = (dx / dist) * force;
		const fy = (dy / dist) * force;
		from.vx += fx; from.vy += fy;
		to.vx   -= fx; to.vy   -= fy;
	}

	// ── Anchor gravity: pull all nodes toward seed at (0,0) ───────────────────
	for (const node of nodes) {
		if (node.isSeed) continue;
		// Cold-start / external nodes sit further out (weaker pull → orbit at edge)
		const strength = ANCHOR_GRAVITY * (node.isColdStart ? 0.35 : 0.9);
		node.vx -= node.x * strength;  // pull toward (0,0) where seed is pinned
		node.vy -= node.y * strength;
	}

	// ── Genre centroid pull (stronger in Genre lens) ──────────────────────────
	const genreStrength = config.genreLensActive ? GENRE_CENTROID_STRENGTH * 2.5 : GENRE_CENTROID_STRENGTH;
	const centroids = computeGenreCentroids(nodes);
	for (const node of nodes) {
		if (node.isSeed) continue;
		const centroid = centroids.get(node.genres[0] ?? node.topGenre ?? 'unknown');
		if (centroid) {
			node.vx += (centroid.x - node.x) * genreStrength;
			node.vy += (centroid.y - node.y) * genreStrength;
		}
	}

	// ── Integrate: pin seed, dampen + clamp others ────────────────────────────
	for (const node of nodes) {
		if (node.isSeed) {
			node.x = node.layoutHint?.x ?? 0;
			node.y = node.layoutHint?.y ?? 0;
			node.vx = 0; node.vy = 0;
			continue;
		}
		if (config.prefersReducedMotion) {
			node.vx = 0; node.vy = 0;
		} else {
			node.vx *= DAMPING;
			node.vy *= DAMPING;
			// Velocity clamp: prevent layout-update lurches. Explicit sqrt is
			// faster than Math.hypot in V8 (Math.hypot pays for overflow handling).
			const speed = Math.sqrt(node.vx * node.vx + node.vy * node.vy);
			if (speed > MAX_SPEED) {
				node.vx = (node.vx / speed) * MAX_SPEED;
				node.vy = (node.vy / speed) * MAX_SPEED;
			}
		}
		node.x += node.vx;
		node.y += node.vy;
	}

	// Positions moved → invalidate hit-test grid (rebuilt lazily on next use).
	invalidateGridCache();
}

// ─── Hover hit-testing ────────────────────────────────────────────────────────

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
	if (nodes.length < DIRECT_REPULSION_THRESHOLD) {
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
