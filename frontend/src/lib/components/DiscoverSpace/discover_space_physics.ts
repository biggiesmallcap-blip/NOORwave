// Force simulation for the DiscoverSpace visualization.
// Pure functions on plain arrays — no Svelte reactivity inside.
// Ported from discoverBuilder.ts and extended with:
//   - Spatial grid bucketing for O(n) repulsion at n > 100
//   - Anchor gravity (seed stays roughly central)
//   - Genre-centroid pull (forms nebula clusters)
//   - Ramping damping (graph settles to slow cosmic drift)
//   - Cold-start / external node weighting

import type { DiscoverTrackNode, DiscoverEdge } from './discover_space_types';

const REPULSION = 2400;
const ATTRACTION = 0.003;
const ANCHOR_GRAVITY = 0.004;
const CENTER_GRAVITY = 0.002;
const GENRE_CENTROID_STRENGTH = 0.006;

// ─── Spatial grid ─────────────────────────────────────────────────────────────

interface SpatialCell {
	nodes: DiscoverTrackNode[];
}

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

function applyGridRepulsion(
	nodes: DiscoverTrackNode[],
	grid: Map<string, SpatialCell>,
	cellSize: number
): void {
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
					const fx = (ddx / dist) * force;
					const fy = (ddy / dist) * force;
					node.vx -= fx * 0.5;
					node.vy -= fy * 0.5;
				}
			}
		}
	}
}

// O(n²) repulsion for small graphs where grid overhead isn't worth it.
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
		entry.x += node.x;
		entry.y += node.y;
		entry.count += 1;
		sums.set(key, entry);
	}
	const centroids = new Map<string, { x: number; y: number }>();
	for (const [key, { x, y, count }] of sums) {
		centroids.set(key, { x: x / count, y: y / count });
	}
	return centroids;
}

// ─── Main force function ──────────────────────────────────────────────────────

export interface PhysicsConfig {
	genreLensActive: boolean;
	prefersReducedMotion: boolean;
}

export function applyForces(
	nodes: DiscoverTrackNode[],
	edges: DiscoverEdge[],
	tick: number,
	config: PhysicsConfig
): void {
	if (nodes.length === 0) return;

	// Damping ramps toward 0.98 over ~300 ticks for "slow cosmic drift" settling.
	const rawDamping = config.prefersReducedMotion
		? 1.0
		: 0.85 + 0.13 * Math.min(1, tick / 300);

	// ── Repulsion ──────────────────────────────────────────────────────────────
	if (nodes.length > 100) {
		const cellSize = 120;
		const grid = buildSpatialGrid(nodes, cellSize);
		applyGridRepulsion(nodes, grid, cellSize);
	} else {
		applyDirectRepulsion(nodes);
	}

	// ── Edge attraction ────────────────────────────────────────────────────────
	const nodeById = new Map(nodes.map((n) => [n.trackId, n]));
	for (const edge of edges) {
		const from = nodeById.get(edge.fromTrackId);
		const to = nodeById.get(edge.toTrackId);
		if (!from || !to) continue;
		const dx = to.x - from.x;
		const dy = to.y - from.y;
		const dist = Math.sqrt(dx * dx + dy * dy) || 1;
		const force = dist * ATTRACTION * edge.weight;
		const fx = (dx / dist) * force;
		const fy = (dy / dist) * force;
		from.vx += fx;
		from.vy += fy;
		to.vx -= fx;
		to.vy -= fy;
	}

	// ── Anchor gravity: soft pull of all nodes toward seed ────────────────────
	const seedNode = nodes.find((n) => n.isSeed);
	if (seedNode) {
		for (const node of nodes) {
			if (node === seedNode) continue;
			// Cold-start / external nodes sit further out (weaker pull).
			const strength = ANCHOR_GRAVITY * (node.isColdStart ? 0.4 : 1.0);
			node.vx += (seedNode.x - node.x) * strength;
			node.vy += (seedNode.y - node.y) * strength;
		}
	}

	// ── Genre centroid pull (stronger when Genre lens active) ─────────────────
	const genreStrength = config.genreLensActive
		? GENRE_CENTROID_STRENGTH * 3
		: GENRE_CENTROID_STRENGTH;
	const centroids = computeGenreCentroids(nodes);
	for (const node of nodes) {
		if (node.isSeed) continue;
		const key = node.genres[0] ?? node.topGenre ?? 'unknown';
		const centroid = centroids.get(key);
		if (centroid) {
			node.vx += (centroid.x - node.x) * genreStrength;
			node.vy += (centroid.y - node.y) * genreStrength;
		}
	}

	// ── Weak center gravity ────────────────────────────────────────────────────
	for (const node of nodes) {
		if (node.isSeed) continue;
		node.vx -= node.x * CENTER_GRAVITY;
		node.vy -= node.y * CENTER_GRAVITY;
	}

	// ── Integrate ──────────────────────────────────────────────────────────────
	for (const node of nodes) {
		if (node.isSeed) {
			// Seed is the world anchor — pin it to the origin
			node.x = 0; node.y = 0; node.vx = 0; node.vy = 0;
			continue;
		}
		if (config.prefersReducedMotion) {
			node.vx = 0;
			node.vy = 0;
		} else {
			node.vx *= rawDamping;
			node.vy *= rawDamping;
		}
		node.x += node.vx;
		node.y += node.vy;
	}
}

// ─── Hover hit-testing ────────────────────────────────────────────────────────

export function findNodeAtPoint(
	nodes: DiscoverTrackNode[],
	worldX: number,
	worldY: number,
	hitRadius = 20
): DiscoverTrackNode | null {
	let best: DiscoverTrackNode | null = null;
	let bestDist = Infinity;
	for (const node of nodes) {
		const dx = node.x - worldX;
		const dy = node.y - worldY;
		const dist = Math.sqrt(dx * dx + dy * dy);
		const threshold = Math.max(node.radius, hitRadius);
		if (dist < threshold && dist < bestDist) {
			best = node;
			bestDist = dist;
		}
	}
	return best;
}

// ─── Spatial grid for hover (faster than O(n) scan) ──────────────────────────

export function findNodeNear(
	nodes: DiscoverTrackNode[],
	worldX: number,
	worldY: number,
	hitRadius = 24
): DiscoverTrackNode | null {
	if (nodes.length < 80) return findNodeAtPoint(nodes, worldX, worldY, hitRadius);

	const cellSize = 80;
	const grid = buildSpatialGrid(nodes, cellSize);
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
				if (dist < threshold && dist < bestDist) {
					best = node;
					bestDist = dist;
				}
			}
		}
	}
	return best;
}
