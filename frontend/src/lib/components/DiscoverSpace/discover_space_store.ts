// Store for the DiscoverSpace visualization. Colocated here (not in lib/stores/)
// to keep all DiscoverSpace code together.

import { get, writable } from 'svelte/store';
import { getApiBase, authFetch } from '$lib/api/client';
import { currentTrack, hydratePlayback } from '$lib/stores/player';
import { showToast } from '$lib/stores/toast';
import { adaptResponse } from './discover_space_adapter';
import type {
	DiscoverTrackNode,
	DiscoverEdge,
	DiscoverLens,
	DiscoverRouteStep,
	VisitedRegion,
	RadioMode,
	ApiDiscoveryResponse,
	DiscoverBlendSeed,
	DiscoverBlendHealth,
} from './discover_space_types';

// In-flight guard. Each loadSpace call increments `loadSpaceSeq` and aborts
// the previous controller, mirroring the pattern in routes/videos/+page.svelte
// (see commit 7ac65cd). A late-arriving response from a now-stale call is
// recognised by its mismatched seq and dropped without touching the store.
let loadSpaceSeq = 0;
let loadSpaceAborter: AbortController | null = null;
let loadBlendSeq = 0;
let loadBlendAborter: AbortController | null = null;

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
	refreshProgress: { stage: string; progress: number } | null;
	blendSeeds: DiscoverBlendSeed[];
	blendHealth: DiscoverBlendHealth | null;
	blendLoading: boolean;
	blendError: string | null;
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
	refreshProgress: null,
	blendSeeds: [],
	blendHealth: null,
	blendLoading: false,
	blendError: null,
});

function blendSeedIdentity(seed: DiscoverBlendSeed): string {
	if (seed.kind === 'library') return `library:${seed.track_id ?? 0}`;
	if (seed.kind === 'tidal') return `tidal:${seed.tidal_id ?? 0}`;
	return `pending:${(seed.artist ?? '').trim().toLowerCase()}:${(seed.title ?? '').trim().toLowerCase()}`;
}

export function normalizeBlendSeeds(seeds: DiscoverBlendSeed[]): DiscoverBlendSeed[] {
	const seen = new Set<string>();
	const unique = seeds
		.map((seed) => ({ ...seed, identity: seed.identity || blendSeedIdentity(seed) }))
		.filter((seed) => {
			if (seen.has(seed.identity)) return false;
			seen.add(seed.identity);
			return true;
		})
		.slice(0, 4);
	const weight = unique.length > 0 ? 1 / unique.length : 0;
	return unique.map((seed) => ({ ...seed, weight }));
}

function blendSeedFromNode(node: DiscoverTrackNode): DiscoverBlendSeed {
	if (node.isInLibrary && node.trackId > 0) {
		return {
			kind: 'library',
			identity: `library:${node.trackId}`,
			track_id: node.trackId,
			title: node.title,
			artist: node.artist,
		};
	}
	const tidalId = node.playable.kind === 'tidal'
		? node.playable.tidal_id
		: node.playability === 'resolvable' && node.trackId > 0
			? node.trackId
			: null;
	if (tidalId != null && tidalId > 0) {
		return {
			kind: 'tidal',
			identity: `tidal:${tidalId}`,
			tidal_id: tidalId,
			title: node.title,
			artist: node.artist,
		};
	}
	return {
		kind: 'pending',
		identity: `pending:${node.artist.trim().toLowerCase()}:${node.title.trim().toLowerCase()}`,
		title: node.title,
		artist: node.artist,
	};
}

export function addBlendSeed(node: DiscoverTrackNode): void {
	discoverSpaceStore.update((s) => ({
		...s,
		blendSeeds: normalizeBlendSeeds([...s.blendSeeds, blendSeedFromNode(node)]),
		blendError: null,
	}));
}

export function removeBlendSeed(identity: string): void {
	discoverSpaceStore.update((s) => {
		const nextSeeds = normalizeBlendSeeds(s.blendSeeds.filter((seed) => seed.identity !== identity));
		if (nextSeeds.length < 2) {
			loadBlendAborter?.abort();
			loadBlendSeq++;
		}
		return {
			...s,
			blendSeeds: nextSeeds,
			blendHealth: nextSeeds.length < 2 ? null : s.blendHealth,
		};
	});
}

export function clearBlend(): void {
	loadBlendAborter?.abort();
	loadBlendSeq++;
	discoverSpaceStore.update((s) => ({
		...s,
		blendSeeds: [],
		blendHealth: null,
		blendLoading: false,
		blendError: null,
	}));
}

function blendRequestBody(seeds: DiscoverBlendSeed[], limit = 100) {
	return JSON.stringify({
		seeds: seeds.map(({ kind, track_id, tidal_id, artist, title, weight }) => ({
			kind,
			track_id,
			tidal_id,
			artist,
			title,
			weight,
		})),
		limit,
	});
}

export async function loadSpace(
	mode: RadioMode,
	seedTrackId: number | undefined,
	prompt: string | undefined,
	seedSource: 'locked' | 'playing' | null,
	currentTrackId: number | null
): Promise<void> {
	loadSpaceAborter?.abort();
	loadBlendAborter?.abort();
	loadBlendSeq++;
	const aborter = new AbortController();
	loadSpaceAborter = aborter;
	const seq = ++loadSpaceSeq;

	discoverSpaceStore.update((s) => {
		// When the active seed changes, drop any in-flight refresh progress —
		// otherwise stale ws messages from the previous seed leave the spinner stuck.
		const seedChanged = s.activeSeedId !== (seedTrackId ?? null);
		return {
			...s,
			loading: true,
			error: null,
			mode,
			activeSeedId: seedTrackId ?? null,
			activeSeedSource: seedSource,
			refreshProgress: seedChanged ? null : s.refreshProgress,
		};
	});

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
			signal: aborter.signal,
		});

		if (seq !== loadSpaceSeq) return;

		if (!response.ok) {
			throw new Error(`Discovery space request failed: ${response.status}`);
		}

		const data: ApiDiscoveryResponse = await response.json();
		if (seq !== loadSpaceSeq) return;

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
		// Aborted requests are expected when a newer call supersedes this one.
		if (e instanceof DOMException && e.name === 'AbortError') return;
		if (seq !== loadSpaceSeq) return;
		const msg = e instanceof Error ? e.message : 'Unknown error';
		console.error('[discoverspace/store] loadSpace failed:', msg);
		discoverSpaceStore.update((s) => ({ ...s, loading: false, error: msg }));
	}
}

export async function loadBlendSpace(currentTrackId: number | null): Promise<void> {
	const seeds = get(discoverSpaceStore).blendSeeds;
	if (seeds.length < 2) {
		loadBlendAborter?.abort();
		loadBlendSeq++;
		discoverSpaceStore.update((s) => ({
			...s,
			blendHealth: null,
			blendLoading: false,
			blendError: null,
		}));
		return;
	}
	loadBlendAborter?.abort();
	const aborter = new AbortController();
	loadBlendAborter = aborter;
	const seq = ++loadBlendSeq;

	discoverSpaceStore.update((s) => ({
		...s,
		blendLoading: true,
		blendError: null,
		blendHealth: null,
		loading: true,
		activeSeedId: null,
		activeSeedSource: null,
	}));

	try {
		const apiBase = getApiBase();
		const response = await authFetch(`${apiBase}/api/discovery/blend/space`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: blendRequestBody(seeds),
			signal: aborter.signal,
		});
		if (seq !== loadBlendSeq) return;
		if (!response.ok) throw new Error(`Blend space request failed: ${response.status}`);
		const data: ApiDiscoveryResponse = await response.json();
		if (seq !== loadBlendSeq) return;
		const { nodes, edges } = adaptResponse(data, currentTrackId, null);
		discoverSpaceStore.update((s) => ({
			...s,
			nodes,
			edges,
			radioRoute: [],
			loading: false,
			blendLoading: false,
			blendHealth: data.health ?? null,
			lastDiagnostics: data.diagnostics ?? null,
		}));
	} catch (e) {
		if (e instanceof DOMException && e.name === 'AbortError') return;
		if (seq !== loadBlendSeq) return;
		const msg = e instanceof Error ? e.message : 'Unknown error';
		console.error('[discoverspace/store] loadBlendSpace failed:', msg);
		discoverSpaceStore.update((s) => ({
			...s,
			loading: false,
			blendLoading: false,
			blendError: msg,
			error: msg,
		}));
	}
}

function playableBlendNodes(nodes: DiscoverTrackNode[]): DiscoverTrackNode[] {
	return nodes
		.filter((node) =>
			node.role !== 'seed'
			&& (node.playability === 'playable' || node.playability === 'resolvable')
		)
		.sort((a, b) => (b.finalBlendScore ?? b.score) - (a.finalBlendScore ?? a.score));
}

async function runBlendQueueAction(
	endpoint: 'add' | 'play' | 'radio',
	currentTrackId: number | null
): Promise<void> {
	const state = get(discoverSpaceStore);
	const seeds = state.blendSeeds;
	if (seeds.length < 2) return;
	const ranked = playableBlendNodes(state.nodes);
	if (ranked.length === 0 && endpoint !== 'add') {
		showToast('No playable blend discoveries yet', 'info');
		return;
	}
	discoverSpaceStore.update((s) => ({ ...s, blendLoading: true, blendError: null }));
	try {
		const apiBase = getApiBase();
		const limit = endpoint === 'radio' ? 200 : 100;
		const response = await authFetch(`${apiBase}/api/discovery/blend/${endpoint}`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: blendRequestBody(seeds, limit),
		});
		if (!response.ok) throw new Error(`Blend action failed: ${response.status}`);
		const result = await response.json();
		if (result.state && result.queue) {
			hydratePlayback({ state: result.state, queue: result.queue });
		}
		if (endpoint === 'play' || endpoint === 'radio') {
			const route = ranked.slice(0, 16).map((node, index) => ({
				trackId: node.trackId,
				reason: node.primaryReason,
				stepIndex: index,
				isCurrent: node.trackId === currentTrackId,
			}));
			discoverSpaceStore.update((s) => ({ ...s, radioRoute: route }));
		}
		showToast(
			endpoint === 'add'
				? 'Added blend discoveries'
				: endpoint === 'play'
					? 'Playing blend discoveries'
					: 'Blend radio started',
			'success'
		);
		discoverSpaceStore.update((s) => ({
			...s,
			blendLoading: false,
			blendHealth: result.health ?? s.blendHealth,
		}));
	} catch (e) {
		const msg = e instanceof Error ? e.message : 'Unknown error';
		discoverSpaceStore.update((s) => ({ ...s, blendLoading: false, blendError: msg }));
		showToast('Blend action failed', 'error');
	}
}

export function addBlendDiscoveries(): Promise<void> {
	return runBlendQueueAction('add', get(currentTrack)?.id ?? null);
}

export function playBlendDiscoveries(): Promise<void> {
	return runBlendQueueAction('play', get(currentTrack)?.id ?? null);
}

export function makeBlendRadio(): Promise<void> {
	return runBlendQueueAction('radio', get(currentTrack)?.id ?? null);
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

export function setRefreshProgress(seedTrackId: number, stage: string, progress: number): void {
	const s = get(discoverSpaceStore);
	if (s.activeSeedId === seedTrackId) {
		discoverSpaceStore.update((st) => ({ ...st, refreshProgress: { stage, progress } }));
	}
}

/// Called by the WebSocket handler when the backend finishes a per-seed neighbor
/// refresh. If the active seed matches, reload the map so new edges appear.
/// The backend's refreshed_seeds set prevents a second refresh from spawning.
export function handleDiscoverySpaceRefreshed(seedTrackId: number): void {
	const s = get(discoverSpaceStore);
	if (s.activeSeedId === seedTrackId) {
		discoverSpaceStore.update((st) => ({ ...st, refreshProgress: null }));
		const track = get(currentTrack);
		loadSpace(s.mode, seedTrackId, undefined, s.activeSeedSource, track?.id ?? null);
	}
}

export function clearRadioRoute(): void {
	discoverSpaceStore.update((s) => ({
		...s,
		radioRoute: [],
		nodes: s.nodes.filter((n) => !n.isRouteOnly),
	}));
}
