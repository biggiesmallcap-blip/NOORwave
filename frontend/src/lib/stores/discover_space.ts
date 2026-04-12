import { writable } from 'svelte/store';
import { getApiBase } from '$lib/api/client';
import type { DiscoverTrackNode, DiscoverEdge, DiscoverViewMode } from '$lib/components/Discover/discover.types';

interface DiscoverSpaceState {
	mode: DiscoverViewMode;
	nodes: DiscoverTrackNode[];
	edges: DiscoverEdge[];
	loading: boolean;
	visitedRegions: Map<string, { x: number; y: number; radius: number }>;
}

export const discoverSpace = writable<DiscoverSpaceState>({
	mode: 'radio',
	nodes: [],
	edges: [],
	loading: false,
	visitedRegions: new Map(),
});

export async function loadSpace(mode: DiscoverViewMode, seedTrackId?: number, prompt?: string) {
	discoverSpace.update(s => ({ ...s, loading: true, mode }));
	try {
		const apiBase = getApiBase();
		const response = await fetch(`${apiBase}/api/discovery/space`, {
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

export function addVisitedRegion(prompt: string, centroid: { x: number; y: number; radius: number }) {
	discoverSpace.update(s => {
		s.visitedRegions.set(prompt, centroid);
		return s;
	});
}
