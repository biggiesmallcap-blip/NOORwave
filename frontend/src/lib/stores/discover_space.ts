import { writable } from 'svelte/store';
import { getApiBase, authFetch } from '$lib/api/client';
import type { DiscoverTrackNode, DiscoverEdge, DiscoverViewMode } from '$lib/components/Discover/discover.types';

interface DiscoverSpaceState {
	mode: DiscoverViewMode;
	nodes: DiscoverTrackNode[];
	edges: DiscoverEdge[];
	loading: boolean;
	visitedRegions: Map<string, { x: number; y: number; radius: number }>;
	// Phase 2: seed management
	lockedSeedId: number | null;     // user-pinned seed; takes precedence over playing
	activeSeedId: number | null;     // resolved seed actually used in the last load
	activeSeedSource: 'locked' | 'playing' | null;
}

export const discoverSpace = writable<DiscoverSpaceState>({
	mode: 'radio',
	nodes: [],
	edges: [],
	loading: false,
	visitedRegions: new Map(),
	lockedSeedId: null,
	activeSeedId: null,
	activeSeedSource: null,
});

export async function loadSpace(
	mode: DiscoverViewMode,
	seedTrackId?: number,
	prompt?: string,
	seedSource: 'locked' | 'playing' | null = null,
) {
	discoverSpace.update(s => ({
		...s,
		loading: true,
		mode,
		activeSeedId: seedTrackId ?? null,
		activeSeedSource: seedSource,
	}));
	try {
		const apiBase = getApiBase();
		const response = await authFetch(`${apiBase}/api/discovery/space`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({
				mode,
				seed_track_id: seedTrackId,
				prompt,
				limit: 60,
				include_artists: mode === 'explore',
			}),
		});

		if (!response.ok) {
			throw new Error(`Failed to load discovery space: ${response.status}`);
		}

		const data = await response.json();
		if (import.meta.env.DEV) {
			console.log('[discover/space] payload', {
				sample_node: data.tracks?.[0],
				sample_edge: data.edges?.[0],
				node_count: data.tracks?.length ?? 0,
				edge_count: data.edges?.length ?? 0,
			});
		}
		discoverSpace.update(s => ({
			...s,
			nodes: data.tracks ?? [],
			edges: data.edges ?? [],
			loading: false,
		}));
	} catch (e) {
		console.error('Failed to load discovery space:', e);
		discoverSpace.update(s => ({ ...s, loading: false }));
	}
}

export function lockSeed(trackId: number) {
	discoverSpace.update(s => ({ ...s, lockedSeedId: trackId }));
}

export function unlockSeed() {
	discoverSpace.update(s => ({ ...s, lockedSeedId: null }));
}

export function addVisitedRegion(prompt: string, centroid: { x: number; y: number; radius: number }) {
	discoverSpace.update(s => {
		s.visitedRegions.set(prompt, centroid);
		return s;
	});
}
