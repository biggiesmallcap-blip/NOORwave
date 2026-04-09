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
	return Math.max(source.depth, target.depth) <= 1 ? 96 : 42;
}

function clampVelocity(node: GalaxyNode) {
	const maxVelocity = 14;
	const velocity = Math.hypot(node.vx, node.vy);
	if (velocity <= maxVelocity || velocity === 0) return;
	const ratio = maxVelocity / velocity;
	node.vx *= ratio;
	node.vy *= ratio;
}

export function runSimulation(nodes: GalaxyNode[], edges: GalaxyEdge[], iterations = 200): GalaxyNode[] {
	if (nodes.length === 0) return nodes;

	const nodeById = new Map(nodes.map((node) => [node.id, node]));
	const rootAnchors = new Map(
		nodes.filter((node) => node.depth === 0).map((node) => [node.id, { x: node.x, y: node.y }])
	);

	for (let tick = 0; tick < iterations; tick += 1) {
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

		for (const edge of edges) {
			const source = nodeById.get(edge.sourceId);
			const target = nodeById.get(edge.targetId);
			if (!source || !target) continue;

			const dx = target.x - source.x;
			const dy = target.y - source.y;
			const distance = Math.max(Math.hypot(dx, dy), 1);
			const targetLength = edge.type === 'sibling' ? 220 : springLength(source, target);
			const stiffness = edge.type === 'sibling' ? 0.0048 : 0.024;
			const displacement = (distance - targetLength) * stiffness;
			const fx = (dx / distance) * displacement;
			const fy = (dy / distance) * displacement;

			source.vx += fx;
			source.vy += fy;
			target.vx -= fx;
			target.vy -= fy;
		}

		const centroids = new Map<number, { x: number; y: number; count: number }>();
		for (const node of nodes) {
			const entry = centroids.get(node.familyId) ?? { x: 0, y: 0, count: 0 };
			entry.x += node.x;
			entry.y += node.y;
			entry.count += 1;
			centroids.set(node.familyId, entry);
		}

		for (const node of nodes) {
			if (node.depth > 0) {
				const centroid = centroids.get(node.familyId);
				if (centroid && centroid.count > 0) {
					const centerX = centroid.x / centroid.count;
					const centerY = centroid.y / centroid.count;
					node.vx += (centerX - node.x) * 0.0033;
					node.vy += (centerY - node.y) * 0.0033;
				}
			}

			if (node.depth === 0) {
				const anchor = rootAnchors.get(node.id);
				if (anchor) {
					node.vx += (anchor.x - node.x) * 0.01;
					node.vy += (anchor.y - node.y) * 0.01;
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
