import type { Genre, GenreHeat, GenreCoOccurrence, GenreCohort, GenreEvolutionPoint, GenreAudioMetrics } from '$lib/api/client';
import {
	GALAXY_ROOT_RING_RADIUS,
	ROOT_FAMILY_COLORS,
	familyKeyFromSlug,
	type GalaxyData,
	type GalaxyEdge,
	type GalaxyNode,
	type RootFamilyKey
} from './galaxy.types';
import { runSimulation } from './simulation';

function nodeRadius(depth: number, trackCount: number): number {
	const base = depth === 0 ? 18 : depth === 1 ? 10 : depth === 2 ? 7 : 5.5;
	const scale = depth === 0 ? 2.8 : depth === 1 ? 1.7 : 1.15;
	const max = depth === 0 ? 36 : depth === 1 ? 20 : 14;
	return Math.min(base + Math.sqrt(trackCount / 10 + 1) * scale, max);
}

function edgeWeight(source: GalaxyNode, target: GalaxyNode, type: GalaxyEdge['type']): number {
	const averageHeat = (source.heatNorm + target.heatNorm) / 2;
	if (type === 'co-listening') return 0.08 + averageHeat * 0.52;
	return type === 'parent-child' ? 0.14 + averageHeat * 0.86 : 0.05 + averageHeat * 0.35;
}

function placeChildren(
	children: Genre[],
	parent: GalaxyNode,
	nodes: GalaxyNode[],
	edges: GalaxyEdge[],
	heatById: Map<number, GenreHeat>,
	maxListenCount: number,
	depth: number,
	familyId: number,
	familyKey: RootFamilyKey,
	familyName: string,
	rootAngle: number,
	metricsById?: Map<number, GenreAudioMetrics>
) {
	if (children.length === 0) return;

	const orderedChildren = [...children].sort((left, right) => {
		const leftCount = left.track_count ?? 0;
		const rightCount = right.track_count ?? 0;
		return rightCount - leftCount || left.name.localeCompare(right.name);
	});
	const spread = depth === 1 ? Math.min(Math.PI * 0.9, 0.72 + orderedChildren.length * 0.14) : Math.min(Math.PI * 0.62, 0.34 + orderedChildren.length * 0.12);
	const anchorAngle = depth === 1 ? rootAngle + Math.PI : Math.atan2(parent.y, parent.x) + Math.PI;
	const radius = depth === 1 ? 148 : depth === 2 ? 74 : 48;

	orderedChildren.forEach((child, index) => {
		const offset =
			orderedChildren.length === 1
				? 0
				: -spread / 2 + (spread * index) / (orderedChildren.length - 1);
		const angle = anchorAngle + offset;
		const heat = heatById.get(child.id);
		const listenCount = heat?.listen_count ?? 0;
		const totalListenedMs = heat?.total_listened_ms ?? 0;
		const heatNorm = maxListenCount > 0 ? Math.log1p(listenCount) / Math.log1p(maxListenCount) : 0;
		const palette = ROOT_FAMILY_COLORS[familyKey];

		const metrics = metricsById?.get(child.id);

		const node: GalaxyNode = {
			id: child.id,
			name: child.name,
			slug: child.slug,
			parentId: child.parent_id,
			familyId,
			familyKey,
			familyName,
			depth,
			trackCount: child.track_count ?? 0,
			listenCount,
			totalListenedMs,
			x: parent.x + Math.cos(angle) * radius,
			y: parent.y + Math.sin(angle) * radius,
			vx: 0,
			vy: 0,
			radius: nodeRadius(depth, child.track_count ?? 0),
			heatNorm,
			color: palette.color,
			glowColor: palette.glowColor,
			orbitRadius: 0,
			cohortId: null,
			evolutionHistory: [],
			avgBpm: metrics?.avg_bpm ?? null,
			avgEnergy: metrics?.avg_energy ?? null,
			avgDanceability: metrics?.avg_danceability ?? null,
		};

		nodes.push(node);
		edges.push({
			sourceId: parent.id,
			targetId: node.id,
			type: 'parent-child',
			weight: 0
		});

		placeChildren(
			child.children ?? [],
			node,
			nodes,
			edges,
			heatById,
			maxListenCount,
			depth + 1,
			familyId,
			familyKey,
			familyName,
			rootAngle,
			metricsById
		);
	});
}

/**
 * Compute orbit radius for each node: genres you listen to frequently
 * orbit closer to center; neglected ones drift outward.
 */
function computeOrbitRadii(nodes: GalaxyNode[], maxListenCount: number): number {
	const maxRadius = GALAXY_ROOT_RING_RADIUS * 0.85;
	const minRadius = 40;
	let maxRadiusUsed = 0;

	nodes.forEach(node => {
		if (maxListenCount === 0) {
			node.orbitRadius = maxRadius;
		} else {
			const inverseHeat = 1 - Math.log1p(node.listenCount) / Math.log1p(maxListenCount);
			node.orbitRadius = minRadius + inverseHeat * (maxRadius - minRadius);
		}
		maxRadiusUsed = Math.max(maxRadiusUsed, node.orbitRadius);
	});

	return maxRadiusUsed;
}

/**
 * Assign cohort IDs to nodes based on cohort data from backend.
 */
function assignCohorts(nodes: GalaxyNode[], cohorts: GenreCohort[]): Map<number, string> {
	const assignment = new Map<number, string>();
	for (const cohort of cohorts) {
		const ids = cohort.genre_ids ?? [];
		for (const genreId of ids) {
			assignment.set(genreId, cohort.id);
		}
	}
	for (const node of nodes) {
		node.cohortId = assignment.get(node.id) ?? null;
	}
	return assignment;
}

/**
 * Attach evolution history to nodes.
 */
function attachEvolution(nodes: GalaxyNode[], evolution: GenreEvolutionPoint[]) {
	const byGenreId = new Map<number, { periodStart: string; listenCount: number }[]>();
	for (const point of evolution) {
		const list = byGenreId.get(point.genre_id) ?? [];
		list.push({ periodStart: point.period_start, listenCount: point.listen_count });
		byGenreId.set(point.genre_id, list);
	}
	// Sort each list chronologically
	for (const [, points] of byGenreId) {
		points.sort((a, b) => a.periodStart.localeCompare(b.periodStart));
	}
	for (const node of nodes) {
		node.evolutionHistory = byGenreId.get(node.id) ?? [];
	}
}

/**
 * Build co-listening edges from backend co-occurrence data.
 */
function buildCoListeningEdges(
	nodes: GalaxyNode[],
	coOccurrences: GenreCoOccurrence[]
): GalaxyEdge[] {
	const nodeById = new Map(nodes.map(n => [n.id, n]));
	const edges: GalaxyEdge[] = [];

	for (const pair of coOccurrences) {
		const nodeA = nodeById.get(pair.genre_a_id);
		const nodeB = nodeById.get(pair.genre_b_id);
		if (!nodeA || !nodeB) continue;

		// Skip if already connected by taxonomy
		const existingTaxonomyEdge = edges.find(
			e => (e.sourceId === nodeA.id && e.targetId === nodeB.id) ||
			     (e.sourceId === nodeB.id && e.targetId === nodeA.id)
		);
		if (existingTaxonomyEdge) continue;

		edges.push({
			sourceId: nodeA.id,
			targetId: nodeB.id,
			type: 'co-listening',
			weight: pair.jaccard
		});
	}

	return edges;
}

export function buildGalaxyData(
	genres: Genre[],
	heat: GenreHeat[],
	options: {
		coOccurrences?: GenreCoOccurrence[];
		cohorts?: GenreCohort[];
		evolution?: GenreEvolutionPoint[];
		metrics?: GenreAudioMetrics[];
		listeningDriven?: boolean;
	} = {}
): GalaxyData {
	if (genres.length === 0) {
		return { nodes: [], edges: [] };
	}

	const { coOccurrences = [], cohorts = [], evolution = [], metrics = [], listeningDriven = false } = options;

	const heatById = new Map(heat.map(entry => [entry.genre_id, entry]));
	const metricsById = metrics.length > 0 ? new Map(metrics.map(m => [m.genre_id, m])) : undefined;
	const maxListenCount = heat.reduce((max, entry) => Math.max(max, entry.listen_count), 0);
	const nodes: GalaxyNode[] = [];
	const edges: GalaxyEdge[] = [];
	const rootCount = genres.length;

	// Phase 1: Compute orbit radii for listening-driven layout
	if (listeningDriven) {
		computeOrbitRadii(nodes, maxListenCount);
	}

	// Assign cohorts
	assignCohorts(nodes, cohorts);

	// Attach evolution history
	attachEvolution(nodes, evolution);

	genres.forEach((root, index) => {
		const familyKey = familyKeyFromSlug(root.slug);
		const palette = ROOT_FAMILY_COLORS[familyKey];
		const rootHeat = heatById.get(root.id);
		const listenCount = rootHeat?.listen_count ?? 0;
		const totalListenedMs = rootHeat?.total_listened_ms ?? 0;
		const heatNorm = maxListenCount > 0 ? Math.log1p(listenCount) / Math.log1p(maxListenCount) : 0;

		let x: number, y: number, angle: number;

		if (listeningDriven) {
			// Distribute on a ring but shift by heat — hotter genres pull toward top
			const baseAngle = (Math.PI * 2 * index) / rootCount;
			const heatOffset = (1 - heatNorm) * Math.PI * 0.15;
			angle = baseAngle + heatOffset;
			const rootOrbitRadius = maxListenCount > 0
				? 200 + (1 - heatNorm) * 180
				: GALAXY_ROOT_RING_RADIUS;
			x = Math.cos(angle) * rootOrbitRadius;
			y = Math.sin(angle) * rootOrbitRadius;
		} else {
			angle = -Math.PI / 2 + (Math.PI * 2 * index) / rootCount;
			x = Math.cos(angle) * GALAXY_ROOT_RING_RADIUS;
			y = Math.sin(angle) * GALAXY_ROOT_RING_RADIUS;
		}

		const rootMetrics = metricsById?.get(root.id);

		const rootNode: GalaxyNode = {
			id: root.id,
			name: root.name,
			slug: root.slug,
			parentId: root.parent_id,
			familyId: root.id,
			familyKey,
			familyName: root.name,
			depth: 0,
			trackCount: root.track_count ?? 0,
			listenCount,
			totalListenedMs,
			x,
			y,
			vx: 0,
			vy: 0,
			radius: nodeRadius(0, root.track_count ?? 0),
			heatNorm,
			color: palette.color,
			glowColor: palette.glowColor,
			orbitRadius: listeningDriven ? (maxListenCount > 0
				? 200 + (1 - heatNorm) * 180
				: GALAXY_ROOT_RING_RADIUS) : 0,
			cohortId: cohorts.find(c => c.genre_ids.includes(root.id))?.id ?? null,
			evolutionHistory: [],
			avgBpm: rootMetrics?.avg_bpm ?? null,
			avgEnergy: rootMetrics?.avg_energy ?? null,
			avgDanceability: rootMetrics?.avg_danceability ?? null,
		};

		nodes.push(rootNode);
		placeChildren(
			root.children ?? [],
			rootNode,
			nodes,
			edges,
			heatById,
			maxListenCount,
			1,
			root.id,
			familyKey,
			root.name,
			listeningDriven ? Math.atan2(y, x) : angle,
			metricsById
		);
	});

	// Attach evolution to root nodes too
	attachEvolution(nodes, evolution);

	// Sibling edges between roots
	const roots = nodes.filter(node => node.depth === 0);
	for (let index = 0; index < roots.length; index += 1) {
		const source = roots[index];
		const target = roots[(index + 1) % roots.length];
		if (!source || !target) continue;
		edges.push({
			sourceId: source.id,
			targetId: target.id,
			type: 'sibling',
			weight: 0
		});
	}

	// Phase 2: Add co-listening edges (emergent cross-genre bridges)
	if (coOccurrences.length > 0) {
		const coEdges = buildCoListeningEdges(nodes, coOccurrences);
		edges.push(...coEdges);
	}

	runSimulation(nodes, edges, 200, { listeningDriven });

	const nodeById = new Map(nodes.map(node => [node.id, node]));
	for (const edge of edges) {
		const source = nodeById.get(edge.sourceId);
		const target = nodeById.get(edge.targetId);
		if (!source || !target) continue;
		edge.weight = edgeWeight(source, target, edge.type);
	}

	return { nodes, edges };
}
