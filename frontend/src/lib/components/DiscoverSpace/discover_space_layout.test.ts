import { describe, expect, it } from 'vitest';
import { applyForces, assignLayoutTargets, R_FAR, R_NEAR } from './discover_space_physics';
import { genreFamily, UNKNOWN_FILL } from './discover_space_genre';
import { nodeColor } from './discover_space_renderer';
import { adaptResponse } from './discover_space_adapter';
import type { DiscoverTrackNode } from './discover_space_types';

function node(trackId: number, overrides: Partial<DiscoverTrackNode> = {}): DiscoverTrackNode {
	return {
		id: `track-${trackId}`,
		trackId,
		title: `Track ${trackId}`,
		artist: 'Artist',
		genres: [],
		score: 0.5,
		confidence: 0.5,
		isSeed: false,
		x: 0,
		y: 0,
		vx: 0,
		vy: 0,
		radius: 10,
		energy: undefined,
		...overrides,
	} as DiscoverTrackNode;
}

const dist = (n: DiscoverTrackNode) => Math.hypot(n.x, n.y);

describe('Sound Space layout', () => {
	it('puts rank on distance: the best match nearest, the weakest furthest', () => {
		const nodes = [
			node(1, { isSeed: true }),
			node(2, { score: 0.2 }),
			node(3, { score: 0.9 }),
			node(4, { score: 0.6 }),
		];
		assignLayoutTargets(nodes, true);
		const [, weak, best, mid] = nodes;
		expect(best!.rank).toBe(0);
		expect(best!.targetR).toBe(R_NEAR);
		expect(weak!.targetR).toBe(R_FAR);
		expect(dist(best!)).toBeLessThan(dist(mid!));
		expect(dist(mid!)).toBeLessThan(dist(weak!));
	});

	it('ranks by the same score the list sorts by (rerank, then shaped)', () => {
		const nodes = [
			node(1, { score: 0.9, shapedScore: 0.2 }),
			node(2, { score: 0.1, shapedScore: 0.3, rerankScore: 0.95 }),
		];
		assignLayoutTargets(nodes, true);
		expect(nodes[1]!.rank).toBe(0);
	});

	it('gives each genre family one contiguous slice, untagged tracks last', () => {
		const rock = [10, 11, 12].map((id) => node(id, { genres: ['Rock'], score: id / 100 }));
		const jazz = [20, 21].map((id) => node(id, { genres: ['Bebop Jazz'], score: id / 100 }));
		const untagged = [node(30, { score: 0.3 })];
		const nodes = [...rock, ...jazz, ...untagged];
		assignLayoutTargets(nodes, false);
		const span = (group: DiscoverTrackNode[]) => {
			const a = group.map((n) => n.targetAngle!);
			return [Math.min(...a), Math.max(...a)];
		};
		const [rockLo, rockHi] = span(rock);
		const [jazzLo, jazzHi] = span(jazz);
		expect(rockHi).toBeLessThan(jazzLo); // bigger family first, no interleaving
		expect(jazzHi).toBeLessThan(untagged[0]!.targetAngle!);
		expect(rockLo).toBeGreaterThanOrEqual(-Math.PI / 2);
	});

	it('places a node once, then only eases it', () => {
		const nodes = [node(1, { score: 0.9 }), node(2, { score: 0.1 })];
		assignLayoutTargets(nodes, true);
		const first = { x: nodes[0]!.x, y: nodes[0]!.y };
		nodes[0]!.rerankScore = 0;
		nodes[1]!.rerankScore = 1;
		assignLayoutTargets(nodes, true);
		expect({ x: nodes[0]!.x, y: nodes[0]!.y }).toEqual(first);
		expect(nodes[0]!.targetR).toBe(R_FAR);
	});

	it('pushes overlapping stars apart', () => {
		const a = node(1, { radius: 12 });
		const b = node(2, { radius: 12 });
		const nodes = [a, b];
		assignLayoutTargets(nodes, true);
		// Force a collision on the same spot.
		b.x = a.x + 1;
		b.y = a.y;
		b.targetR = a.targetR;
		b.targetAngle = a.targetAngle;
		for (let i = 0; i < 200; i++) applyForces(nodes, [], { genreLensActive: false, prefersReducedMotion: false });
		expect(Math.hypot(a.x - b.x, a.y - b.y)).toBeGreaterThanOrEqual(24);
	});
});

describe('genre families', () => {
	it('matches the most specific family first', () => {
		expect(genreFamily('Pop Punk')?.key).toBe('punk');
		expect(genreFamily('Indie Pop')?.key).toBe('indie');
		expect(genreFamily('Trip-Hop')?.key).toBe('electronic');
		expect(genreFamily('Dubstep')?.key).toBe('electronic');
		expect(genreFamily('Reggaeton')?.key).toBe('latin');
		expect(genreFamily('Garage Rock')?.key).toBe('rock');
		expect(genreFamily('Dancehall')?.key).toBe('reggae');
		expect(genreFamily(null)).toBeNull();
		expect(genreFamily('Polka')).toBeNull();
	});
});

describe('lens colours', () => {
	it('draws missing data grey instead of a made-up value', () => {
		expect(nodeColor(node(1), 'energy')).toBe(UNKNOWN_FILL);
		expect(nodeColor(node(1, { energy: 0.8 }), 'energy')).not.toBe(UNKNOWN_FILL);
		expect(nodeColor(node(1), 'genre')).toBe(UNKNOWN_FILL);
		expect(nodeColor(node(1, { genres: ['Jazz'] }), 'genre')).not.toBe(UNKNOWN_FILL);
	});
});

describe('reasons from the server', () => {
	it('keeps already-normalized reasons instead of reading them as unknown', () => {
		const reasons = ['external', 'energy', 'artist', 'genre', 'bpm', 'album', 'similar', 'behavioral'];
		const { nodes } = adaptResponse(
			{
				tracks: reasons.map((primary_reason, i) => ({
					track_id: i + 2, title: 'T', artist_name: 'A', primary_reason, score: 0.5, confidence: 0.5,
				})) as never,
			},
			null,
			1,
		);
		expect(nodes.map((n) => n.primaryReason)).toEqual(reasons);
	});
});

describe('layout origin and reduced motion (review of #371)', () => {
	const settle = (nodes: DiscoverTrackNode[], reduced: boolean, ticks: number) => {
		for (let i = 0; i < ticks; i++) applyForces(nodes, [], { genreLensActive: false, prefersReducedMotion: reduced });
	};
	const twenty = () => [
		node(1, { isSeed: true, layoutHint: { x: 80, y: 0 } }),
		...Array.from({ length: 20 }, (_, i) => node(i + 2, { score: 1 - i * 0.04, radius: 12 })),
	];
	const fromSeed = (seed: DiscoverTrackNode, n: DiscoverTrackNode) => Math.hypot(n.x - seed.x, n.y - seed.y);

	it('centres the seed, the rank targets and the rings on one origin', () => {
		const nodes = twenty();
		assignLayoutTargets(nodes, true);
		settle(nodes, false, 400);
		const seed = nodes[0]!;
		expect(seed.x).toBe(0);
		expect(seed.y).toBe(0);
		const stars = nodes.slice(1).toSorted((a, b) => a.rank! - b.rank!);
		for (const n of stars) expect(Math.abs(fromSeed(seed, n) - n.targetR!)).toBeLessThan(16);
		// Rank order survives on distance wherever targets are 2+ ranks apart.
		for (let i = 0; i < stars.length; i++) {
			for (let j = i + 2; j < stars.length; j++) {
				expect(fromSeed(seed, stars[i]!)).toBeLessThan(fromSeed(seed, stars[j]!));
			}
		}
	});

	it('finishes a rerank in one reduced-motion call', () => {
		const nodes = twenty();
		assignLayoutTargets(nodes, true);
		settle(nodes, true, 1);
		// Reverse the order, as a run of Like/Skip could.
		nodes.slice(1).forEach((n, i) => { n.rerankScore = i; });
		assignLayoutTargets(nodes, true);
		settle(nodes, true, 1); // the canvas settles right after this call
		const seed = nodes[0]!;
		for (const n of nodes.slice(1)) expect(Math.abs(fromSeed(seed, n) - n.targetR!)).toBeLessThan(16);
	});

	it('keeps blend seeds spread around the shared origin', () => {
		const nodes = [
			node(1, { isSeed: true, layoutHint: { x: 100, y: 0 } }),
			node(2, { isSeed: true, layoutHint: { x: -60, y: 0 } }),
			node(3, { score: 0.5 }),
		];
		assignLayoutTargets(nodes, true);
		expect(nodes[0]!.x + nodes[1]!.x).toBeCloseTo(0);
		expect(nodes[0]!.x - nodes[1]!.x).toBeCloseTo(160);
	});
});
