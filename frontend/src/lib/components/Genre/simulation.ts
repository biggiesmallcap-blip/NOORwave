import type { GalaxyEdge, GalaxyNode } from './galaxy.types';

const ROOT_REPULSION_MULTIPLIER = 6;
const DEPTH_ONE_REPULSION_MULTIPLIER = 2.4;
const DEEP_REPULSION_MULTIPLIER = 1.35;
const DAMPING = 0.85;

function repulsionForDepth(depth: number): number {
	if (depth === 0) return ROOT_REPULSION_MULTIPLIER;
	if (depth === 1) return DEPTH_ONE_REPULSION_MULTIPLIER;
	return DEEP_REPULSION_MULTIPLIER;
}

function springLength(source: GalaxyNode, target: GalaxyNode): number {
	if (source.depth === 0 && target.depth === 0) return 220;
	if (source.depth <= 1 && target.depth <= 1) return 96;
	return 42;
}

function clampVelocity(node: GalaxyNode) {
	const maxVelocity = 14;
	const velocity = Math.hypot(node.vx, node.vy);
	if (velocity <= maxVelocity || velocity === 0) return;
	const ratio = maxVelocity / velocity;
	node.vx *= ratio;
	node.vy *= ratio;
}

export function runSimulation(
	nodes: GalaxyNode[],
	edges: GalaxyEdge[],
	iterations = 200,
	options: { listeningDriven?: boolean } = {}
): GalaxyNode[] {
	if (nodes.length === 0) return nodes;

	const { listeningDriven = false } = options;
	const nodeById = new Map(nodes.map(node => [node.id, node]));
	const rootAnchors = new Map(
		nodes.filter(node => node.depth === 0).map(node => [node.id, { x: node.x, y: node.y }])
	);

	// Pre-compute cohort centroids for Phase 2
	const cohortCentroids = new Map<string, { x: number; y: number; count: number }>();
	if (listeningDriven) {
		for (const node of nodes) {
			if (!node.cohortId) continue;
			const entry = cohortCentroids.get(node.cohortId) ?? { x: 0, y: 0, count: 0 };
			entry.x += node.x;
			entry.y += node.y;
			entry.count += 1;
			cohortCentroids.set(node.cohortId, entry);
		}
	}

	// Pre-compute co-listening edge list for gravity
	const coListeningEdges = edges.filter(e => e.type === 'co-listening');

	for (let tick = 0; tick < iterations; tick += 1) {
		// All-pairs repulsion
		for (let i = 0; i < nodes.length; i += 1) {
			const a = nodes[i];
			for (let j = i + 1; j < nodes.length; j += 1) {
				const b = nodes[j];
				const dx = b.x - a.x;
				const dy = b.y - a.y;
				const distanceSq = Math.max(dx * dx + dy * dy, 36);
				const distance = Math.sqrt(distanceSq);
				const charge = (140 * repulsionForDepth(a.depth) * repulsionForDepth(b.depth)) / distanceSq;
				const fx = (dx / distance) * charge;
				const fy = (dy / distance) * charge;

				a.vx -= fx;
				a.vy -= fy;
				b.vx += fx;
				b.vy += fy;
			}
		}

		// Spring attraction for all edges
		for (const edge of edges) {
			const source = nodeById.get(edge.sourceId);
			const target = nodeById.get(edge.targetId);
			if (!source || !target) continue;

			const dx = target.x - source.x;
			const dy = target.y - source.y;
			const distance = Math.max(Math.hypot(dx, dy), 1);

			let targetLength: number;
			let stiffness: number;

			if (edge.type === 'co-listening') {
				// Co-listening edges: stronger pull for higher jaccard
				targetLength = 120 - edge.weight * 60; // 60-120px range
				stiffness = 0.012 * edge.weight; // weighted by co-listening strength
			} else if (edge.type === 'sibling') {
				targetLength = 220;
				stiffness = 0.0048;
			} else {
				targetLength = springLength(source, target);
				stiffness = 0.024;
			}

			const displacement = (distance - targetLength) * stiffness;
			const fx = (dx / distance) * displacement;
			const fy = (dy / distance) * displacement;

			source.vx += fx;
			source.vy += fy;
			target.vx -= fx;
			target.vy -= fy;
		}

		// Family centroids (taxonomy-based grouping)
		const familyCentroids = new Map<number, { x: number; y: number; count: number }>();
		for (const node of nodes) {
			const entry = familyCentroids.get(node.familyId) ?? { x: 0, y: 0, count: 0 };
			entry.x += node.x;
			entry.y += node.y;
			entry.count += 1;
			familyCentroids.set(node.familyId, entry);
		}

		for (const node of nodes) {
			// Phase 1: Listening-driven orbital pull toward center
			if (listeningDriven && node.orbitRadius > 0) {
				const distFromCenter = Math.hypot(node.x, node.y);
				const targetDist = node.orbitRadius;
				const orbitalPull = (targetDist - distFromCenter) * 0.0018;
				if (distFromCenter > 0) {
					node.vx += (node.x / distFromCenter) * orbitalPull;
					node.vy += (node.y / distFromCenter) * orbitalPull;
				}
			}

			// Phase 2: Cohort gravity pull
			if (listeningDriven && node.cohortId) {
				const centroid = cohortCentroids.get(node.cohortId);
				if (centroid && centroid.count > 1) {
					const centerX = centroid.x / centroid.count;
					const centerY = centroid.y / centroid.count;
					node.vx += (centerX - node.x) * 0.0022;
					node.vy += (centerY - node.y) * 0.0022;
				}
			}

			// Taxonomy family pull (always, weaker when listening-driven)
			if (node.depth > 0) {
				const centroid = familyCentroids.get(node.familyId);
				if (centroid && centroid.count > 0) {
					const centerX = centroid.x / centroid.count;
					const centerY = centroid.y / centroid.count;
					const pullStrength = listeningDriven ? 0.0012 : 0.0033;
					node.vx += (centerX - node.x) * pullStrength;
					node.vy += (centerY - node.y) * pullStrength;
				}
			}

			// Root anchoring
			if (node.depth === 0) {
				const anchor = rootAnchors.get(node.id);
				if (anchor) {
					const anchorStrength = listeningDriven ? 0.006 : 0.01;
					node.vx += (anchor.x - node.x) * anchorStrength;
					node.vy += (anchor.y - node.y) * anchorStrength;
				}
			}

			node.vx *= DAMPING;
			node.vy *= DAMPING;
			clampVelocity(node);
			node.x += node.vx;
			node.y += node.vy;
		}
	}

	return nodes;
}
