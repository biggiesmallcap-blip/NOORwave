import { describe, expect, test, beforeEach, vi, afterEach } from 'vitest';
import { get } from 'svelte/store';

// Mock the API client BEFORE importing the store, so the store's
// `import { authFetch } from '$lib/api/client'` resolves to the mock.
const authFetchMock = vi.fn();
vi.mock('$lib/api/client', () => ({
	getApiBase: () => 'http://test',
	authFetch: (...args: unknown[]) => authFetchMock(...args),
}));

import {
	addBlendSeed,
	clearBlend,
	discoverSpaceStore,
	loadBlendSpace,
	loadSpace,
	removeBlendSeed,
} from './discover_space_store';
import type { DiscoverTrackNode } from './discover_space_types';

function libraryNode(trackId: number, title = `Seed ${trackId}`): DiscoverTrackNode {
	return {
		id: `track-${trackId}`,
		trackId,
		title,
		artist: 'Artist',
		playable: {
			kind: 'library',
			track_id: trackId,
			track: {} as any,
		},
		source: 'library',
		role: 'library_guide',
		playability: 'playable',
		isInLibrary: true,
		isColdStart: false,
		genres: [],
		score: 1,
		confidence: 1,
		supportCount: 0,
		inDegree: 0,
		inDegreePctile: 0,
		primaryReason: 'unknown',
		reasonTags: [],
		perSeedScores: [],
		coverageBonus: 0,
		externalBonus: 0,
		libraryPenalty: 0,
		isSeed: false,
		isPlaying: false,
		inPlaylistBuilder: false,
		isRouteOnly: false,
		x: 0,
		y: 0,
		vx: 0,
		vy: 0,
		radius: 10,
	};
}

function deferredJson(seedId: number) {
	let resolve!: (v: unknown) => void;
	const promise = new Promise((r) => {
		resolve = r;
	});
	const response = {
		ok: true,
		json: () =>
			Promise.resolve({
				diagnostics: { seed_id: seedId },
				nodes: [],
				edges: [],
				artists: [],
				generated_at: new Date().toISOString(),
				seed_track_id: seedId,
			}),
	};
	return { promise, resolve: () => resolve(response) };
}

function deferredBlendJson() {
	let resolve!: (v: unknown) => void;
	const promise = new Promise((r) => {
		resolve = r;
	});
	const response = {
		ok: true,
		json: () =>
			Promise.resolve({
				tracks: [
					{
						track_id: 900,
						title: 'Stale Blend',
						artist_name: 'Artist',
						is_in_library: false,
						role: 'external_candidate',
						playability: 'resolvable',
						score: 0.9,
					},
				],
				edges: [],
				artists: [],
				health: {
					playable_external_count: 1,
					pending_external_count: 0,
					library_guide_count: 0,
					coverage_ratio: 1,
				},
				generated_at: new Date().toISOString(),
			}),
	};
	return { promise, resolve: () => resolve(response) };
}

describe('loadSpace', () => {
	beforeEach(() => {
		authFetchMock.mockReset();
		discoverSpaceStore.set({
			mode: 'radio',
			nodes: [],
			edges: [],
			radioRoute: [],
			visitedRegions: [],
			lens: 'energy',
			loading: false,
			error: null,
			lockedSeedId: null,
			activeSeedId: null,
			activeSeedSource: null,
			lastDiagnostics: null,
			refreshProgress: null,
			blendSeeds: [],
			blendHealth: null,
			blendLoading: false,
			blendError: null,
			coherence: 0.5,
			filters: {},
			sessionId: '',
			feedbackBusy: false,
		});
	});

	afterEach(() => {
		vi.restoreAllMocks();
	});

	test('a second loadSpace call aborts the in-flight first call', async () => {
		const first = deferredJson(111);
		const second = deferredJson(222);

		// First call: capture the AbortSignal handed to authFetch.
		let firstSignal: AbortSignal | undefined;
		authFetchMock.mockImplementationOnce((_url, init) => {
			firstSignal = (init as RequestInit).signal as AbortSignal;
			return first.promise;
		});
		// Second call: resolve immediately so the store settles on seed 222.
		authFetchMock.mockImplementationOnce(() => second.promise);

		const p1 = loadSpace('radio', 111, undefined, 'locked', null);
		const p2 = loadSpace('radio', 222, undefined, 'locked', null);

		// Settle the second call first — its result is the one the user expects.
		second.resolve();
		await p2;

		expect(firstSignal?.aborted).toBe(true);
		expect(get(discoverSpaceStore).activeSeedId).toBe(222);

		// Now resolve the first (stale) response. It must NOT clobber the store.
		first.resolve();
		await p1;

		expect(get(discoverSpaceStore).activeSeedId).toBe(222);
	});

	test('addBlendSeed removes duplicates and assigns equal weights', () => {
		addBlendSeed(libraryNode(1, 'Seed A'));
		addBlendSeed(libraryNode(1, 'Seed A duplicate'));
		addBlendSeed(libraryNode(2, 'Seed B'));

		const seeds = get(discoverSpaceStore).blendSeeds;
		expect(seeds).toHaveLength(2);
		expect(seeds.map((seed) => seed.identity)).toEqual(['library:1', 'library:2']);
		expect(seeds[0]?.weight).toBeCloseTo(0.5);
		expect(seeds[1]?.weight).toBeCloseTo(0.5);
	});

	test('blend seed add remove and clear update store state', () => {
		addBlendSeed({
			id: 'track-10',
			trackId: 10,
			title: 'Seed A',
			artist: 'Artist',
			playable: {
				kind: 'library',
				track_id: 10,
				track: {} as any,
			},
			source: 'library',
			role: 'library_guide',
			playability: 'playable',
			isInLibrary: true,
			isColdStart: false,
			genres: [],
			score: 1,
			confidence: 1,
			supportCount: 0,
			inDegree: 0,
			inDegreePctile: 0,
			primaryReason: 'unknown',
			reasonTags: [],
			perSeedScores: [],
			coverageBonus: 0,
			externalBonus: 0,
			libraryPenalty: 0,
			isSeed: false,
			isPlaying: false,
			inPlaylistBuilder: false,
			isRouteOnly: false,
			x: 0,
			y: 0,
			vx: 0,
			vy: 0,
			radius: 10,
		});
		expect(get(discoverSpaceStore).blendSeeds).toHaveLength(1);

		removeBlendSeed('library:10');
		expect(get(discoverSpaceStore).blendSeeds).toHaveLength(0);

		addBlendSeed({
			id: 'track-11',
			trackId: 11,
			title: 'Seed B',
			artist: 'Artist',
			playable: {
				kind: 'library',
				track_id: 11,
				track: {} as any,
			},
			source: 'library',
			role: 'library_guide',
			playability: 'playable',
			isInLibrary: true,
			isColdStart: false,
			genres: [],
			score: 1,
			confidence: 1,
			supportCount: 0,
			inDegree: 0,
			inDegreePctile: 0,
			primaryReason: 'unknown',
			reasonTags: [],
			perSeedScores: [],
			coverageBonus: 0,
			externalBonus: 0,
			libraryPenalty: 0,
			isSeed: false,
			isPlaying: false,
			inPlaylistBuilder: false,
			isRouteOnly: false,
			x: 0,
			y: 0,
			vx: 0,
			vy: 0,
			radius: 10,
		});
		clearBlend();
		expect(get(discoverSpaceStore).blendSeeds).toHaveLength(0);
	});

	test('loadBlendSpace ignores fewer than two unique seeds', async () => {
		addBlendSeed({
			id: 'track-12',
			trackId: 12,
			title: 'Seed C',
			artist: 'Artist',
			playable: {
				kind: 'library',
				track_id: 12,
				track: {} as any,
			},
			source: 'library',
			role: 'library_guide',
			playability: 'playable',
			isInLibrary: true,
			isColdStart: false,
			genres: [],
			score: 1,
			confidence: 1,
			supportCount: 0,
			inDegree: 0,
			inDegreePctile: 0,
			primaryReason: 'unknown',
			reasonTags: [],
			perSeedScores: [],
			coverageBonus: 0,
			externalBonus: 0,
			libraryPenalty: 0,
			isSeed: false,
			isPlaying: false,
			inPlaylistBuilder: false,
			isRouteOnly: false,
			x: 0,
			y: 0,
			vx: 0,
			vy: 0,
			radius: 10,
		});

		await loadBlendSpace(null);

		expect(authFetchMock).not.toHaveBeenCalled();
		expect(get(discoverSpaceStore).blendHealth).toBeNull();
		expect(get(discoverSpaceStore).blendLoading).toBe(false);
	});

	test('clearBlend invalidates an in-flight blend response', async () => {
		discoverSpaceStore.update((s) => ({
			...s,
			nodes: [],
			blendSeeds: [
				{ kind: 'library', identity: 'library:1', track_id: 1, title: 'Seed A', weight: 0.5 },
				{ kind: 'library', identity: 'library:2', track_id: 2, title: 'Seed B', weight: 0.5 },
			],
		}));
		const blend = deferredBlendJson();
		authFetchMock.mockImplementationOnce(() => blend.promise);

		const request = loadBlendSpace(null);
		clearBlend();
		blend.resolve();
		await request;

		const state = get(discoverSpaceStore);
		expect(state.blendSeeds).toHaveLength(0);
		expect(state.blendHealth).toBeNull();
		expect(state.nodes).toHaveLength(0);
	});
});
