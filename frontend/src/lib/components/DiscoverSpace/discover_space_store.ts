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
	DiscoverFilters,
	BranchStep,
} from './discover_space_types';
import { isFilterNoop } from './discover_space_types';

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
	coherence: number;
	filters: DiscoverFilters;
	sessionId: string;
	feedbackBusy: boolean;
	branchPath: BranchStep[];
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
	coherence: 0.5,
	filters: {},
	sessionId: '',
	feedbackBusy: false,
	branchPath: [],
});

// -- Control persistence (sessionStorage) -------------------------------------
// Hydrated from +page.svelte onMount (SSR-safe); written through on change so
// navigating away and back keeps the user's coherence, filters, and session.

const CONTROLS_STORAGE_KEY = 'discoverspace.controls.v1';
const SESSION_STORAGE_KEY = 'discoverspace.session.v1';
const BRANCH_STORAGE_KEY = 'discoverspace.branch.v1';

function persistBranch(branchPath: BranchStep[], lockedSeedId: number | null): void {
	try {
		sessionStorage.setItem(BRANCH_STORAGE_KEY, JSON.stringify({ branchPath, lockedSeedId }));
	} catch {
		// Storage unavailable - the tree just resets next visit.
	}
}

function persistControls(coherence: number, filters: DiscoverFilters): void {
	try {
		sessionStorage.setItem(CONTROLS_STORAGE_KEY, JSON.stringify({ coherence, filters }));
	} catch {
		// Storage unavailable (private mode etc.) - controls just reset next visit.
	}
}

export function hydrateDiscoverControls(): void {
	let coherence = 0.5;
	let filters: DiscoverFilters = {};
	let sessionId = '';
	let branchPath: BranchStep[] = [];
	let lockedSeedId: number | null = null;
	try {
		const rawControls = sessionStorage.getItem(CONTROLS_STORAGE_KEY);
		if (rawControls) {
			const parsed = JSON.parse(rawControls);
			if (typeof parsed.coherence === 'number') {
				coherence = Math.min(1, Math.max(0, parsed.coherence));
			}
			if (parsed.filters && typeof parsed.filters === 'object') filters = parsed.filters;
		}
		sessionId = sessionStorage.getItem(SESSION_STORAGE_KEY) ?? '';
		const rawBranch = sessionStorage.getItem(BRANCH_STORAGE_KEY);
		if (rawBranch) {
			const parsed = JSON.parse(rawBranch);
			if (Array.isArray(parsed.branchPath)) branchPath = parsed.branchPath;
			if (typeof parsed.lockedSeedId === 'number') lockedSeedId = parsed.lockedSeedId;
		}
	} catch {
		// Fall through to defaults.
	}
	if (!sessionId) {
		sessionId =
			typeof crypto !== 'undefined' && 'randomUUID' in crypto
				? crypto.randomUUID()
				: `s-${Date.now()}-${Math.floor(Math.random() * 1e9)}`;
		try {
			sessionStorage.setItem(SESSION_STORAGE_KEY, sessionId);
		} catch {
			// Non-persistent session id still works for this page lifetime.
		}
	}
	discoverSpaceStore.update((s) => ({
		...s,
		coherence,
		filters,
		sessionId,
		branchPath,
		// Restoring the lock restores the tree position: the page's seed effect
		// sees the locked id and loads that space.
		lockedSeedId: lockedSeedId ?? s.lockedSeedId,
	}));
}

/// Reload whichever space is active (blend when 2+ seeds, seed space
/// otherwise) with the current store controls. Used by the coherence slider
/// and filter bar.
function reloadActiveSpace(): void {
	const s = get(discoverSpaceStore);
	const track = get(currentTrack);
	if (s.blendSeeds.length >= 2) {
		loadBlendSpace(track?.id ?? null);
	} else if (s.activeSeedId !== null) {
		loadSpace(s.mode, s.activeSeedId, undefined, s.activeSeedSource, track?.id ?? null);
	}
}

let coherenceReloadTimer: ReturnType<typeof setTimeout> | null = null;

export function setCoherence(value: number): void {
	const coherence = Math.min(1, Math.max(0, value));
	const s = get(discoverSpaceStore);
	discoverSpaceStore.update((st) => ({ ...st, coherence }));
	persistControls(coherence, s.filters);
	// Debounced reload: the slider fires continuously while dragging.
	if (coherenceReloadTimer) clearTimeout(coherenceReloadTimer);
	coherenceReloadTimer = setTimeout(() => {
		coherenceReloadTimer = null;
		reloadActiveSpace();
	}, 300);
}

export function setFilters(filters: DiscoverFilters): void {
	const s = get(discoverSpaceStore);
	discoverSpaceStore.update((st) => ({ ...st, filters }));
	persistControls(s.coherence, filters);
	reloadActiveSpace();
}

function blendSeedIdentity(seed: DiscoverBlendSeed): string {
	if (seed.kind === 'library') return `library:${seed.track_id ?? 0}`;
	if (seed.kind === 'tidal') return `tidal:${seed.tidal_id ?? 0}`;
	return `pending:${(seed.artist ?? '').trim().toLowerCase()}:${(seed.title ?? '').trim().toLowerCase()}`;
}

function normalizeBlendSeeds(seeds: DiscoverBlendSeed[]): DiscoverBlendSeed[] {
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

// addBlendSeed / removeBlendSeed are the SOLE triggers of the blend fetch;
// callers must not also invoke loadBlendSpace or the request fires twice.

export function addBlendSeed(node: DiscoverTrackNode): void {
	discoverSpaceStore.update((s) => ({
		...s,
		blendSeeds: normalizeBlendSeeds([...s.blendSeeds, blendSeedFromNode(node)]),
		blendError: null,
	}));
	if (get(discoverSpaceStore).blendSeeds.length >= 2) {
		loadBlendSpace(get(currentTrack)?.id ?? null);
	}
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
	if (get(discoverSpaceStore).blendSeeds.length >= 2) {
		loadBlendSpace(get(currentTrack)?.id ?? null);
	}
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

/// Current control fields for any discovery request body. Filters are omitted
/// entirely when no-op so the wire payload stays byte-compatible with the
/// pre-controls contract.
function controlRequestFields() {
	const s = get(discoverSpaceStore);
	return {
		coherence: s.coherence,
		session_id: s.sessionId || undefined,
		filters: isFilterNoop(s.filters) ? undefined : s.filters,
	};
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
		...controlRequestFields(),
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
				...controlRequestFields(),
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

// -- Branching -------------------------------------------------------------
// Branching is lock-plus-history: the node becomes the locked seed (the
// page's seed effect reloads the space) and the seed we were on joins the
// breadcrumb path so the user can walk back up the tree.

/// Make `node` the new seed, remembering where we branched from.
export function branchHere(node: DiscoverTrackNode): void {
	discoverSpaceStore.update((s) => {
		let branchPath = s.branchPath;
		const fromId = s.activeSeedId;
		if (fromId !== null && fromId !== node.trackId) {
			const from = s.nodes.find((n) => n.trackId === fromId);
			const step: BranchStep = {
				seedTrackId: fromId,
				title: from?.title ?? `Track ${fromId}`,
				artist: from?.artist ?? '',
			};
			// Re-branching to a seed already in the path truncates instead of
			// looping (walking A > B > A keeps the tree a path, not a cycle).
			const existing = branchPath.findIndex((b) => b.seedTrackId === node.trackId);
			branchPath =
				existing >= 0 ? branchPath.slice(0, existing) : [...branchPath, step];
		}
		persistBranch(branchPath, node.trackId);
		return { ...s, branchPath, lockedSeedId: node.trackId };
	});
}

/// Walk back to a breadcrumb step, dropping everything after it.
export function walkBack(index: number): void {
	discoverSpaceStore.update((s) => {
		const step = s.branchPath[index];
		if (!step) return s;
		const branchPath = s.branchPath.slice(0, index);
		persistBranch(branchPath, step.seedTrackId);
		return { ...s, branchPath, lockedSeedId: step.seedTrackId };
	});
}

/// Drop the whole branch history (keeps the current seed).
export function clearBranchPath(): void {
	discoverSpaceStore.update((s) => {
		persistBranch([], s.lockedSeedId);
		return { ...s, branchPath: [] };
	});
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

// -- Like/skip feedback + stateless rerank -------------------------------------

async function sendFeedbackAndRerank(
	node: DiscoverTrackNode,
	action: 'like' | 'skip'
): Promise<void> {
	const s = get(discoverSpaceStore);
	if (s.feedbackBusy) return;
	discoverSpaceStore.update((st) => ({ ...st, feedbackBusy: true }));
	try {
		const apiBase = getApiBase();
		const feedbackResp = await authFetch(`${apiBase}/api/discovery/feedback`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({
				seed_track_id: s.activeSeedId ?? 0,
				candidate_track_id: node.trackId,
				action,
				surface: 'discover_space',
				session_id: s.sessionId || undefined,
			}),
		});
		if (!feedbackResp.ok) throw new Error(`Feedback failed: ${feedbackResp.status}`);

		// Rerank the full non-seed set against the updated session taste.
		// base_score is the PRE-shaping raw score so shaping runs exactly once.
		const candidates = get(discoverSpaceStore)
			.nodes.filter((n) => !n.isSeed)
			.map((n) => ({
				track_id: n.trackId,
				is_in_library: n.isInLibrary,
				base_score: n.rawScore ?? n.score,
				artist_name: n.artist,
			}));
		const rerankResp = await authFetch(`${apiBase}/api/discovery/rerank`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({
				session_id: s.sessionId || undefined,
				seed_track_id: s.activeSeedId ?? undefined,
				coherence: s.coherence,
				candidates,
			}),
		});
		if (!rerankResp.ok) throw new Error(`Rerank failed: ${rerankResp.status}`);
		const result = await rerankResp.json();
		const byId = new Map<number, { score: number; why: string; why_signals: string[] }>(
			(result.scores ?? []).map((row: { track_id: number; score: number; why: string; why_signals: string[] }) => [
				row.track_id,
				row,
			])
		);
		// Merge scores/why only; canvas positions and normalized display score
		// stay put so the map does not jump on every like.
		discoverSpaceStore.update((st) => ({
			...st,
			nodes: st.nodes.map((n) => {
				const row = byId.get(n.trackId);
				return row
					? { ...n, rerankScore: row.score, why: row.why, whySignals: row.why_signals }
					: n;
			}),
		}));
	} catch (e) {
		const msg = e instanceof Error ? e.message : 'Unknown error';
		console.error('[discoverspace/store] feedback failed:', msg);
		showToast(action === 'like' ? 'Could not record like' : 'Could not record skip', 'error');
	} finally {
		discoverSpaceStore.update((st) => ({ ...st, feedbackBusy: false }));
	}
}

export function likeNode(node: DiscoverTrackNode): Promise<void> {
	return sendFeedbackAndRerank(node, 'like');
}

export function skipNode(node: DiscoverTrackNode): Promise<void> {
	return sendFeedbackAndRerank(node, 'skip');
}

// -- Queue-all / play-all through the pending-queue pipeline --------------------

function queueItemFromNode(node: DiscoverTrackNode) {
	const base = {
		artist: node.artist,
		title: node.title,
		reason: node.why || node.primaryReason,
		score: node.shapedScore ?? node.score,
	};
	if (node.playable.kind === 'library') {
		return { ...base, track_id: node.playable.track_id, is_in_library: true };
	}
	if (node.playable.kind === 'tidal') {
		return { ...base, tidal_id: node.playable.tidal_id };
	}
	if (node.playable.kind === 'pending-lastfm') {
		return base;
	}
	return null; // unavailable
}

export async function queueSpaceTracks(
	nodes: DiscoverTrackNode[],
	play: boolean
): Promise<void> {
	const items = nodes
		.map(queueItemFromNode)
		.filter((item): item is NonNullable<typeof item> => item !== null);
	if (items.length === 0) {
		showToast('Nothing playable to queue', 'info');
		return;
	}
	try {
		const apiBase = getApiBase();
		const response = await authFetch(`${apiBase}/api/discovery/space/queue`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ items, play }),
		});
		if (!response.ok) throw new Error(`Queue request failed: ${response.status}`);
		const result = await response.json();
		if (play && result.state && result.queue) {
			hydratePlayback({ state: result.state, queue: result.queue });
		}
		const count = result.queued_count ?? items.length;
		showToast(play ? `Playing ${count} tracks` : `Queued ${count} tracks`, 'success');
	} catch (e) {
		const msg = e instanceof Error ? e.message : 'Unknown error';
		console.error('[discoverspace/store] queueSpaceTracks failed:', msg);
		showToast('Could not queue tracks', 'error');
	}
}
