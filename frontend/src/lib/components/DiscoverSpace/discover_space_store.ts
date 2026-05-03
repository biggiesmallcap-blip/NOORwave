// Store for the DiscoverSpace visualization. Colocated here (not in lib/stores/)
// to keep all DiscoverSpace code together.

import { writable } from 'svelte/store';
import { getApiBase, authFetch } from '$lib/api/client';
import { adaptResponse } from './discover_space_adapter';
import type {
	DiscoverTrackNode,
	DiscoverEdge,
	DiscoverLens,
	DiscoverRouteStep,
	VisitedRegion,
	RadioMode,
	ApiDiscoveryResponse,
} from './discover_space_types';

interface DiscoverSpaceState {
	mode: RadioMode;
	nodes: DiscoverTrackNode[];
	edges: DiscoverEdge[];
	radioRoute: DiscoverRouteStep[];
	visitedRegions: VisitedRegion[];
	lens: DiscoverLens;
	loading: boolean;
	error: string | null;
	lockedSeedId: number | null;
	activeSeedId: number | null;
	activeSeedSource: 'locked' | 'playing' | null;
	lastDiagnostics: ApiDiscoveryResponse['diagnostics'] | null;
}

export const discoverSpaceStore = writable<DiscoverSpaceState>({
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
});

export async function loadSpace(
	mode: RadioMode,
	seedTrackId: number | undefined,
	prompt: string | undefined,
	seedSource: 'locked' | 'playing' | null,
	currentTrackId: number | null
): Promise<void> {
	discoverSpaceStore.update((s) => ({
		...s,
		loading: true,
		error: null,
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
				limit: 100,
				include_artists: mode === 'explore',
			}),
		});

		if (!response.ok) {
			throw new Error(`Discovery space request failed: ${response.status}`);
		}

		const data: ApiDiscoveryResponse = await response.json();
		const { nodes, edges } = adaptResponse(data, currentTrackId, seedTrackId ?? null);

		if (import.meta.env.DEV) {
			console.log('[discoverspace/store] loaded', {
				nodes: nodes.length,
				edges: edges.length,
				diagnostics: data.diagnostics,
			});
		}

		discoverSpaceStore.update((s) => ({
			...s,
			nodes,
			edges,
			loading: false,
			lastDiagnostics: data.diagnostics ?? null,
		}));
	} catch (e) {
		const msg = e instanceof Error ? e.message : 'Unknown error';
		console.error('[discoverspace/store] loadSpace failed:', msg);
		discoverSpaceStore.update((s) => ({ ...s, loading: false, error: msg }));
	}
}

export function lockSeed(trackId: number): void {
	discoverSpaceStore.update((s) => ({ ...s, lockedSeedId: trackId }));
}

export function unlockSeed(): void {
	discoverSpaceStore.update((s) => ({ ...s, lockedSeedId: null }));
}

export function setLens(lens: DiscoverLens): void {
	discoverSpaceStore.update((s) => ({ ...s, lens }));
}

export function addVisitedRegion(label: string, centroid: VisitedRegion['centroid']): void {
	discoverSpaceStore.update((s) => ({
		...s,
		visitedRegions: [...s.visitedRegions.filter((r) => r.label !== label), { label, centroid }],
	}));
}

export function setRadioRoute(steps: DiscoverRouteStep[], ghostNodes?: DiscoverTrackNode[]): void {
	discoverSpaceStore.update((s) => {
		const existingIds = new Set(s.nodes.map((n) => n.trackId));
		const newNodes = ghostNodes?.filter((n) => !existingIds.has(n.trackId)) ?? [];
		return {
			...s,
			radioRoute: steps,
			nodes: newNodes.length > 0 ? [...s.nodes, ...newNodes] : s.nodes,
		};
	});
}

export function clearRadioRoute(): void {
	discoverSpaceStore.update((s) => ({
		...s,
		radioRoute: [],
		nodes: s.nodes.filter((n) => !n.isRouteOnly),
	}));
}
