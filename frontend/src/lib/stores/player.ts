import { get, writable } from 'svelte/store';
import {
	api,
	ApiError,
	type AudioDspFeatures,
	type PendingCandidateInfo,
	type PlaybackRuntimeInfo,
	type PlaybackSnapshot,
	type PlaybackState,
	type QueueItem,
	type RadioBlend,
	type StreamDisplayInfo,
	type TidalDiscographyTrack,
	type TidalPlayable,
	type Track
} from '$lib/api/client';
import { setExclusiveEngaged, setExclusiveReleased } from '$lib/stores/exclusive_status';
import { showToast, dismissToast } from '$lib/stores/toast';
import { announceQueue, announceResolved } from '$lib/stores/queue_announcer';
import { offerUndo } from '$lib/stores/queue_undo';
import {
	albumEntryStartIndex,
	albumEntryToMixedQueueItem,
	libraryTrackToMixedQueueItem,
	mergeAlbumTracks,
	queueItemToTidalPlayable,
	tidalPlayableToMixedQueueItem,
} from '$lib/utils/track';
import { currentQueueAnchorItem } from '$lib/player/queue_active';
import { wsConnected } from '$lib/api/ws';
import { updateLibraryTrackFavorite } from '$lib/stores/library';
import { clamp01 } from '$lib/utils/math';

function trackLabel(track: { title?: string | null; artist_name?: string | null }): string {
	const t = (track.title ?? '').trim();
	const a = (track.artist_name ?? '').trim();
	if (t && a) return `${t} — ${a}`;
	return t || a || 'track';
}

export const currentTrack = writable<Track | null>(null);
export const currentQueueItemId = writable<number | null>(null);
export const currentTrackFeatures = writable<AudioDspFeatures | null>(null);
export const currentStreamDisplay = writable<StreamDisplayInfo | null>(null);
export const playbackRuntimeInfo = writable<PlaybackRuntimeInfo | null>(null);

// TIDAL tracks only carry artist/album tidal ids in this in-memory cache; the
// backend queue snapshots may omit them before a pending row resolves. Persisting it to localStorage
// keeps the now-playing artist/album links alive across the Tauri WebView2
// reload (and any navigation), which would otherwise wipe the Map and strip the
// links off whatever is playing. Stale entries are harmless: they're keyed by
// tidal_id and only consulted when a matching track surfaces.
const TIDAL_META_CACHE_KEY = 'noor_tidal_meta_cache_v1';
const TIDAL_META_CACHE_MAX = 800;

function loadTidalMetaCache(): Map<number, Partial<TidalPlayable>> {
	if (typeof localStorage === 'undefined') return new Map();
	try {
		const raw = localStorage.getItem(TIDAL_META_CACHE_KEY);
		if (!raw) return new Map();
		const entries = JSON.parse(raw) as [number, Partial<TidalPlayable>][];
		return new Map(Array.isArray(entries) ? entries.slice(-TIDAL_META_CACHE_MAX) : []);
	} catch {
		return new Map();
	}
}

let persistMetaTimer: ReturnType<typeof setTimeout> | null = null;
function persistTidalMetaCacheSoon() {
	if (typeof localStorage === 'undefined' || persistMetaTimer) return;
	persistMetaTimer = setTimeout(() => {
		persistMetaTimer = null;
		try {
			// Map keeps insertion order, so trimming from the front drops the
			// oldest entries when we exceed the cap.
			while (tidalMetadataById.size > TIDAL_META_CACHE_MAX) {
				const oldest = tidalMetadataById.keys().next().value;
				if (oldest === undefined) break;
				tidalMetadataById.delete(oldest);
			}
			localStorage.setItem(TIDAL_META_CACHE_KEY, JSON.stringify([...tidalMetadataById.entries()]));
		} catch {
			// Quota/serialization failure is non-fatal: links just won't survive a reload.
		}
	}, 500);
}

const tidalMetadataById = loadTidalMetaCache();
const tidalFavoriteOverrideById = new Map<number, { localId: number; favorite: boolean }>();

type TidalMetadataInput = Pick<TidalPlayable, 'tidal_id' | 'title'> & Partial<Omit<TidalPlayable, 'tidal_id' | 'title'>>;

function localTidalTrackId(track: Pick<TidalPlayable, 'tidal_id'> & Partial<TidalPlayable>): number | null {
	const id = track.track_id ?? track.local_id ?? null;
	return typeof id === 'number' && id > 0 ? id : null;
}

function rememberTidalPlayable(track: TidalMetadataInput) {
	if (!track.tidal_id || track.tidal_id <= 0) return;
	const previous = tidalMetadataById.get(track.tidal_id) ?? {};
	tidalMetadataById.set(track.tidal_id, {
		...previous,
		artist_name: track.artist_name ?? previous.artist_name ?? null,
		artist_tidal_id: track.artist_tidal_id ?? previous.artist_tidal_id ?? null,
		album_title: track.album_title ?? previous.album_title ?? null,
		album_tidal_id: track.album_tidal_id ?? previous.album_tidal_id ?? null,
		artwork_url: track.artwork_url ?? previous.artwork_url ?? null,
		duration_ms: track.duration_ms ?? previous.duration_ms ?? null,
		track_id: track.track_id ?? previous.track_id,
		local_id: track.local_id ?? previous.local_id ?? null,
		is_in_library: track.is_in_library ?? previous.is_in_library,
		is_favorite: track.is_favorite ?? previous.is_favorite,
	});
	persistTidalMetaCacheSoon();
}

function rememberTidalPlayables(tracks: readonly TidalMetadataInput[]) {
	for (const track of tracks) rememberTidalPlayable(track);
}

function enrichTidalTrack(track: Track | null): Track | null {
	if (!track?.tidal_id) return track;
	const cached = tidalMetadataById.get(track.tidal_id);
	if (!cached) return track;
	const localId = localTidalTrackId({ tidal_id: track.tidal_id, ...cached });
	const isLocalTrack = track.id > 0;
	const effectiveId = localId ?? track.id;
	const favoriteOverride = tidalFavoriteOverrideById.get(track.tidal_id);
	const isFavorite =
		favoriteOverride && favoriteOverride.localId === effectiveId
			? favoriteOverride.favorite
			: isLocalTrack
				? track.is_favorite
				: (cached.is_favorite ?? track.is_favorite);
	return {
		...track,
		id: effectiveId,
		artist_name: track.artist_name ?? cached.artist_name ?? null,
		artist_tidal_id: track.artist_tidal_id ?? cached.artist_tidal_id ?? null,
		album_title: track.album_title ?? cached.album_title ?? null,
		album_tidal_id: track.album_tidal_id ?? cached.album_tidal_id ?? null,
		artwork_url: track.artwork_url ?? cached.artwork_url ?? null,
		duration_ms: track.duration_ms ?? cached.duration_ms ?? null,
		is_favorite: isFavorite,
	};
}

function enrichQueue(queue: QueueItem[]): QueueItem[] {
	return queue.map((item) => ({
		...item,
		track: enrichTidalTrack(item.track) ?? item.track,
	}));
}

function setCurrentTrack(track: Track | null) {
	currentTrack.set(enrichTidalTrack(track));
}

function setPlaybackQueue(queue: QueueItem[]) {
	const enriched = enrichQueue(queue);
	const previous = get(playbackQueue);
	const resolvedDelta = countResolvedTransitions(previous, enriched);
	playbackQueue.set(enriched);
	if (resolvedDelta > 0) announceResolved(resolvedDelta);
}

// Count rows whose persisted queue id existed in `previous` as pending and now
// resolved in `next`. Newly added rows and removed rows are ignored: the live
// region only surfaces *transitions* so initial hydration and reorderings stay
// quiet.
export function countResolvedTransitions(previous: QueueItem[], next: QueueItem[]): number {
	if (previous.length === 0) return 0;
	const wasPending = new Set<number>();
	for (const item of previous) {
		if (item.is_pending === true) wasPending.add(item.id);
	}
	if (wasPending.size === 0) return 0;
	let resolved = 0;
	for (const item of next) {
		if (item.is_pending !== true && wasPending.has(item.id)) resolved += 1;
	}
	return resolved;
}

// ─── Error model ──────────────────────────────────────────────────────────────
// `playerError` holds a friendly message + optional retry callback. The layout
// renders a toast with auto-dismiss; the store stays inert so we can replace
// the message at any time without juggling timers in here.
export interface PlayerError {
	message: string;
	retry?: () => Promise<void>;
}

export const playerError = writable<PlayerError | null>(null);

// Tracks the last successful API call so `assertOnline` can avoid bouncing the
// user when the WS happens to be transiently disconnected but the HTTP path is
// healthy.
export const lastSuccessfulCallAt = writable<number>(0);
const ONLINE_GRACE_MS = 30_000;

function noteSuccess() {
	lastSuccessfulCallAt.set(Date.now());
}

let playbackIntentSeq = 0;
let activePlaybackIntentSeq: number | null = null;

function beginPlaybackIntent(): number {
	playbackIntentSeq += 1;
	activePlaybackIntentSeq = playbackIntentSeq;
	return playbackIntentSeq;
}

function finishPlaybackIntent(seq: number) {
	if (activePlaybackIntentSeq === seq) activePlaybackIntentSeq = null;
}

function isLatestPlaybackIntent(seq: number): boolean {
	return seq === playbackIntentSeq;
}

function currentPlaybackIntentSeq(): number {
	return playbackIntentSeq;
}

function shouldApplyPassivePlaybackSnapshot(seq: number): boolean {
	return seq === playbackIntentSeq && activePlaybackIntentSeq === null;
}

function applyStateIfLatest(state: PlaybackState, seq: number): boolean {
	if (!isLatestPlaybackIntent(seq)) return false;
	applyState(state);
	return true;
}

function hydratePlaybackIfLatest(snapshot: PlaybackSnapshot, seq: number): boolean {
	if (!isLatestPlaybackIntent(seq)) return false;
	hydratePlayback(snapshot);
	return true;
}

/**
 * Map an arbitrary error into a short, friendly message. The raw `error` is
 * dropped in production so we never leak `TypeError: Failed to fetch` style
 * goo into the UI; we keep it appended in dev for debugging convenience.
 */
export function normalizePlayerError(action: string, error: unknown): string {
	const raw = error instanceof Error ? error.message : String(error ?? '');
	const lower = raw.toLowerCase();

	if (lower.includes('failed to fetch') || lower.includes('networkerror') || lower.includes('network error')) {
		return "Can't reach the server. Check it's running.";
	}
	if (lower.includes('timed out') || lower.includes('timeout')) {
		return 'Server took too long. Try again.';
	}
	if (lower.includes('tidal not connected')) {
		return 'Tidal disconnected — re-auth in Settings.';
	}
	if (/\b(401|403)\b/.test(raw) || lower.includes('unauthorized') || lower.includes('forbidden')) {
		return 'Session expired — sign in again.';
	}
	if (/\b404\b/.test(raw) || lower.includes('not found')) {
		return "We couldn't find that.";
	}
	if (/\b5\d\d\b/.test(raw)) {
		return 'Server hiccup. Try that again in a moment.';
	}

	const fallback = `Couldn't ${action}.`;
	if (import.meta.env.DEV && raw) {
		return `${fallback} (${raw})`;
	}
	return fallback;
}

function setError(action: string, error: unknown, retry?: () => Promise<void>) {
	playerError.set({ message: normalizePlayerError(action, error), retry });
}

/**
 * Pre-flight gate for heavy actions. Returns true if the WebSocket is open OR
 * a successful HTTP call landed within the last 30 s. Otherwise sets a
 * "Reconnecting…" toast and returns false so the caller can bail without
 * firing a request that's likely to hang.
 */
export function assertOnline(): boolean {
	if (get(wsConnected)) return true;
	if (Date.now() - get(lastSuccessfulCallAt) < ONLINE_GRACE_MS) return true;
	playerError.set({ message: 'Reconnecting to the server…' });
	return false;
}

export async function refreshPlaybackRuntime() {
	try {
		const result = await api.getPlaybackRuntime();
		currentStreamDisplay.set(result.stream ?? null);
		playbackRuntimeInfo.set(result.runtime ?? null);
		// Sync exclusive status store so the pill shows correctly after page load
		// without waiting for the next WS exclusive event.
		if (result.runtime) {
			if (result.runtime.exclusive_engaged) {
				setExclusiveEngaged(
					result.runtime.device_name,
					result.runtime.exclusive_transport_format
				);
			} else {
				setExclusiveReleased(result.runtime.device_name ?? '');
			}
		}
		noteSuccess();
	} catch {
		// non-critical — playback still works without live stream info
	}
}

// ─── Current-track DSP features fetcher ───────────────────────────────────────
// Listens for track-id changes on currentTrack and fetches audio features in
// the background. Errors are swallowed — playback must never block on this.
let _lastFeaturesTrackId: number | null = null;
let _featuresFetchSeq = 0;

function fetchCurrentTrackFeatures(trackId: number, clearFirst: boolean): void {
	const seq = ++_featuresFetchSeq;
	if (clearFirst) {
		// Clear stale features immediately so UI doesn't show the previous track's badge.
		currentTrackFeatures.set(null);
	}
	void api
		.getTrackAudioFeatures(trackId)
		.then((res) => {
			// Guard against out-of-order responses.
			if (seq !== _featuresFetchSeq) return;
			currentTrackFeatures.set(res.features ?? null);
		})
		.catch(() => {
			if (seq !== _featuresFetchSeq) return;
			currentTrackFeatures.set(null);
		});
}

currentTrack.subscribe((track) => {
	const nextId = track?.id ?? null;
	if (nextId === _lastFeaturesTrackId) return;
	_lastFeaturesTrackId = nextId;

	if (nextId === null) {
		currentTrackFeatures.set(null);
		return;
	}

	fetchCurrentTrackFeatures(nextId, true);
});

// A passive DSP analysis or queue prescan just stamped fresh features for some
// track. If it's the one currently playing, refresh in place so the badge picks
// up the new BPM/key/Camelot without waiting for a track change.
if (typeof window !== 'undefined') {
	window.addEventListener('noor:dsp_updated', (event) => {
		const trackId = (event as CustomEvent<{ trackId: number }>).detail?.trackId;
		if (typeof trackId !== 'number') return;
		if (trackId !== _lastFeaturesTrackId) return;
		fetchCurrentTrackFeatures(trackId, false);
	});
}

export const isPlaying = writable(false);
export const position = writable(0);
/**
 * How many ms of the current track are decoded into the playback buffer.
 * Drives the buffered-bar overlay in the scrubber and clamps the user's
 * seek max so a drag past the loaded region is impossible (the route-side
 * 409 ack is the backstop). Updated by `applyState` + a 1 Hz refresher
 * that polls `/api/playback/state` while a track is loading.
 */
export const buffered = writable(0);
export const volume = writable(1.0);
export const volumeBeforeMute = writable<number | null>(null);
export const automixEnabled = writable(false);
export const automixDiscoverNew = writable(false);
export const automixUseLearning = writable(true);
export const automixAllowExternal = writable(false);

// ─── Client-side position ticker ──────────────────────────────────────────────
// Uses performance.now() timestamps instead of counting ticks so that browsers
// throttling setInterval in hidden tabs doesn't cause drift — the position is
// always computed as (last-synced value) + (actual elapsed wall-clock time).
let _positionTicker: ReturnType<typeof setInterval> | null = null;
let _tickerBasePosition = 0;   // position_ms at the last server sync or start
let _tickerBaseTime = 0;       // performance.now() at that sync point

export function anchorPositionTicker(positionMs: number) {
	_tickerBasePosition = positionMs;
	_tickerBaseTime = performance.now();
}

function startPositionTicker() {
	if (_positionTicker !== null) return;
	_tickerBaseTime = performance.now();
	_positionTicker = setInterval(() => {
		const track = get(currentTrack);
		if (!track?.duration_ms) return;
		const elapsed = performance.now() - _tickerBaseTime;
		position.set(Math.min(_tickerBasePosition + elapsed, track.duration_ms));
	}, 250);
}

function stopPositionTicker() {
	if (_positionTicker !== null) {
		clearInterval(_positionTicker);
		_positionTicker = null;
	}
}

isPlaying.subscribe((playing) => {
	if (playing) {
		startPositionTicker();
	} else {
		stopPositionTicker();
	}
});

// ─── 1 Hz buffered refresher ─────────────────────────────────────────────────
// While a track is active and the decoder hasn't caught up to the duration,
// poll `/api/playback/state` once per second so the buffered-bar overlay
// grows live. Auto-stops when buffered >= duration, when the track changes,
// or when no track is active. The `PlaybackBuffer` on the server only
// appends (never compacts mid-track), so the `>=` stop condition is sound.
let _bufferedRefresher: ReturnType<typeof setTimeout> | null = null;
const BUFFERED_REFRESH_INTERVAL_MS = 1000;

function clearBufferedRefresher() {
	if (_bufferedRefresher !== null) {
		clearTimeout(_bufferedRefresher);
		_bufferedRefresher = null;
	}
}

function scheduleBufferedRefreshIfNeeded() {
	clearBufferedRefresher();
	const track = get(currentTrack);
	const duration = track?.duration_ms ?? 0;
	if (!track || duration <= 0) return;
	if (get(buffered) >= duration) return;
	const refreshSeq = currentPlaybackIntentSeq();
	_bufferedRefresher = setTimeout(async () => {
		try {
			const snapshot = await api.getPlaybackState();
			if (!shouldApplyPassivePlaybackSnapshot(refreshSeq)) return;
			// applyState writes `buffered` and recurses into scheduling, so
			// the timer chain runs as long as buffered_ms < duration_ms.
			applyState(snapshot.state);
		} catch (_err) {
			// Transient fetch failures (sleep, network blip) shouldn't toast.
			// Re-schedule so we recover on the next interval if the track is
			// still active when connectivity returns.
			scheduleBufferedRefreshIfNeeded();
		}
	}, BUFFERED_REFRESH_INTERVAL_MS);
}
export const shuffleMode = writable<PlaybackState['shuffle_mode']>('off');
export const repeatMode = writable<PlaybackState['repeat_mode']>('off');
export const crossfadeMs = writable(0);
export const playbackQueue = writable<QueueItem[]>([]);
export const playerReady = writable(false);

// Map track_id → human-readable "why this track is here" string, populated when
// a radio orchestrator returns candidates. The server attaches a `reason` to
// every RadioCandidate but our queue endpoint rebuilds QueueItems server-side
// from track ids, dropping the reason. Keep a client-side map so the queue
// panel can surface it.
export const radioReasons = writable<Record<number, string>>({});

function setRadioReasons(entries: { track_id: number; reason?: string | null }[]) {
	const next: Record<number, string> = {};
	for (const e of entries) {
		if (e.reason && e.track_id > 0) next[e.track_id] = e.reason;
	}
	radioReasons.set(next);
}

function clearRadioReasons() {
	radioReasons.set({});
}

// Cycle: off → genre (Galaxy default) → weighted → true → back to off
const SHUFFLE_SEQUENCE: PlaybackState['shuffle_mode'][] = ['off', 'genre', 'weighted', 'true'];

function applyState(state: PlaybackState) {
	setCurrentTrack(state.current_track);
	currentQueueItemId.set(state.current_queue_item_id ?? null);
	isPlaying.set(state.is_playing);
	position.set(state.position_ms);
	anchorPositionTicker(state.position_ms);
	buffered.set(state.buffered_ms ?? 0);
	volume.set(state.volume);
	shuffleMode.set(state.shuffle_mode);
	repeatMode.set(state.repeat_mode);
	automixEnabled.set(state.automix_enabled);
	crossfadeMs.set(state.crossfade_ms);
	automixDiscoverNew.set(state.automix_discover_new);
	automixUseLearning.set(state.automix_use_learning);
	automixAllowExternal.set(state.automix_allow_external);
	scheduleBufferedRefreshIfNeeded();
}

function resetOptimisticPlaybackProgress() {
	clearBufferedRefresher();
	position.set(0);
	anchorPositionTicker(0);
	buffered.set(0);
}

export function hydratePlayback(snapshot: PlaybackSnapshot) {
	applyState(snapshot.state);
	setPlaybackQueue(snapshot.queue);
	playerReady.set(true);
	playerError.set(null);
	noteSuccess();
}

export async function refreshPlaybackState() {
	playerError.set(null);
	const refreshSeq = currentPlaybackIntentSeq();
	try {
		const snapshot = await api.getPlaybackState();
		if (!shouldApplyPassivePlaybackSnapshot(refreshSeq)) return;
		hydratePlayback(snapshot);
	} catch (error) {
		if (!shouldApplyPassivePlaybackSnapshot(refreshSeq)) return;
		setError('load playback state', error, () => refreshPlaybackState());
	}
}

export async function playTrackNow(trackId: number) {
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const snapshot = await api.playTrack(trackId);
		hydratePlaybackIfLatest(snapshot, intentSeq);
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('play that track', error, () => playTrackNow(trackId));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}
export async function playQueueItemNow(queueItemId: number) {
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const snapshot = await api.playQueueItem(queueItemId);
		hydratePlaybackIfLatest(snapshot, intentSeq);
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('play that queue item', error, () => playQueueItemNow(queueItemId));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

/**
 * The action the most recent still-in-flight toggle asked for. Rapid clicks
 * alternate off THIS - the thing the user just requested - instead of the
 * `isPlaying` store, which lags while responses and WS snapshots are still
 * landing. That store lag was the "stale isPlaying flipped my command inside
 * out" race: mash the button during a slow track transition and pause/resume
 * commands came out inverted, leaving the button saying paused while audio
 * kept playing.
 */
let pendingToggleAction: 'pause' | 'resume' | null = null;

export async function togglePlayback() {
	playerError.set(null);
	// Decide the intended action ONCE, from the freshest signal available:
	// the previous in-flight toggle if there is one, else the current store.
	const intended: 'pause' | 'resume' = pendingToggleAction
		? pendingToggleAction === 'pause'
			? 'resume'
			: 'pause'
		: get(isPlaying)
			? 'pause'
			: 'resume';
	pendingToggleAction = intended;
	// Instant button feedback; the response snapshot below (and the WS-driven
	// authoritative state pushes) reconcile to the server's truth.
	isPlaying.set(intended === 'resume');
	const intentSeq = beginPlaybackIntent();
	try {
		const result =
			intended === 'pause' ? await api.pausePlayback() : await api.resumePlayback();
		if (!applyStateIfLatest(result.state, intentSeq)) return;
		noteSuccess();
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('toggle playback', error, () => togglePlayback());
	} finally {
		if (pendingToggleAction === intended && isLatestPlaybackIntent(intentSeq)) {
			pendingToggleAction = null;
		}
		finishPlaybackIntent(intentSeq);
	}
}

/**
 * Explicit pause/resume helpers for callers that have a definite intent (e.g.
 * MediaSession lockscreen / headset buttons, or sleep timer). These skip the
 * toggle's alternation logic entirely: the caller already knows the action.
 */
export async function pausePlayer() {
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const result = await api.pausePlayback();
		if (!applyStateIfLatest(result.state, intentSeq)) return;
		noteSuccess();
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('pause playback', error, () => pausePlayer());
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function resumePlayer() {
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const result = await api.resumePlayback();
		if (!applyStateIfLatest(result.state, intentSeq)) return;
		noteSuccess();
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('resume playback', error, () => resumePlayer());
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function playPreviousTrack() {
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const snapshot = await api.previousTrack();
		hydratePlaybackIfLatest(snapshot, intentSeq);
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('go to previous track', error, () => playPreviousTrack());
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function playNextTrack() {
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		// Optimistic update: show the next queued track immediately rather than waiting
		// for TIDAL stream resolution (~2-5s). hydratePlayback below corrects any mismatch.
		const currentRepeat = get(repeatMode);
		if (currentRepeat !== 'one') {
			const queue = get(playbackQueue);
			const current = get(currentTrack);
			const nextItem = selectOptimisticNextItem(
				queue,
				current?.id ?? null,
				get(currentQueueItemId)
			);
			if (nextItem) {
				setCurrentTrack(nextItem.track);
			}
		}
		position.set(0);
		anchorPositionTicker(0);

		const snapshot = await api.nextTrack();
		hydratePlaybackIfLatest(snapshot, intentSeq);
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('skip to next track', error, () => playNextTrack());
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function setPlayerVolume(nextVolume: number) {
	playerError.set(null);
	try {
		const clamped = clamp01(nextVolume);
		const result = await api.setPlaybackVolume(clamped);
		// Only sync volume — applying full state would overwrite the local position
		// ticker with a slightly stale server value, causing the displayed time to jump.
		volume.set(result.state.volume);
		noteSuccess();
	} catch (error) {
		setError('set volume', error);
	}
}

export async function toggleMute() {
	const current = get(volume);
	if (current > 0) {
		volumeBeforeMute.set(current);
		await setPlayerVolume(0);
	} else {
		const restore = get(volumeBeforeMute) ?? 0.5;
		volumeBeforeMute.set(null);
		await setPlayerVolume(restore);
	}
}

export async function setPlayerPosition(nextPositionMs: number) {
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		// Always opt in to the segment-restart path (option C). With
		// allow_segment_seek=true the backend treats out-of-buffer targets
		// as a forced restart at the nearest DASH segment instead of a 409.
		// Pre-resolve / non-DASH targets still get 409 (the catch below
		// applies the corrective snapshot); transition errors get 500
		// (treat as recoverable error - the user can retry the drag).
		const result = await api.setPlaybackPosition(nextPositionMs, true);
		if (!applyStateIfLatest(result.state, intentSeq)) return;
		noteSuccess();
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		// HTTP 409 = pre-resolve race or unparseable manifest (no segment
		// offsets to restart at). Apply the corrective live snapshot the
		// server included in the body so the scrubber visibly snaps back,
		// stay silent (no error toast).
		if (error instanceof ApiError && error.status === 409) {
			const body = error.body as { state?: PlaybackState } | null;
			if (body?.state) {
				if (!applyStateIfLatest(body.state, intentSeq)) return;
				noteSuccess();
				return;
			}
		}
		// HTTP 500 = segment-restart transition errored (TIDAL re-resolve
		// failed, decoder spin-up failed, etc.). Show error toast with a
		// retry; this is a recoverable failure, not a programming error.
		setError('seek', error, () => setPlayerPosition(nextPositionMs));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function setPlayerRepeatMode(mode: PlaybackState['repeat_mode']) {
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const result = await api.setPlaybackRepeat(mode);
		if (!applyStateIfLatest(result.state, intentSeq)) return;
		noteSuccess();
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('set repeat mode', error, () => setPlayerRepeatMode(mode));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function cyclePlayerRepeatMode() {
	const sequence: PlaybackState['repeat_mode'][] = ['off', 'all', 'one'];
	const current = get(repeatMode);
	const next = sequence[(sequence.indexOf(current) + 1) % sequence.length];
	await setPlayerRepeatMode(next);
}

export async function setPlayerShuffleMode(mode: PlaybackState['shuffle_mode']) {
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const snapshot = await api.setPlaybackShuffle(mode);
		if (!hydratePlaybackIfLatest(snapshot, intentSeq)) return null;
		return snapshot;
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return null;
		setError('set shuffle mode', error, async () => {
			await setPlayerShuffleMode(mode);
		});
		return null;
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function setPlayerAutomixEnabled(
	enabled: boolean,
	crossfade_ms?: number,
	discover_new?: boolean,
	use_learning?: boolean,
	allow_external?: boolean
) {
	playerError.set(null);
	try {
		const result = await api.setPlaybackAutomix(
			enabled,
			crossfade_ms,
			discover_new,
			use_learning,
			allow_external
		);
		// Only sync automix fields — applying full state would clobber the local position ticker.
		automixEnabled.set(result.state.automix_enabled);
		crossfadeMs.set(result.state.crossfade_ms);
		automixDiscoverNew.set(result.state.automix_discover_new);
		automixUseLearning.set(result.state.automix_use_learning);
		automixAllowExternal.set(result.state.automix_allow_external);
		if (result.queue) setPlaybackQueue(result.queue);
		noteSuccess();
	} catch (error) {
		setError('update automix', error, () =>
			setPlayerAutomixEnabled(enabled, crossfade_ms, discover_new, use_learning, allow_external)
		);
	}
}

export async function setPlayerCrossfadeMs(ms: number) {
	await setPlayerAutomixEnabled(get(automixEnabled), ms);
}

export async function setPlayerDiscoverNew(enabled: boolean) {
	await setPlayerAutomixEnabled(get(automixEnabled), get(crossfadeMs), enabled);
}

export async function setPlayerAutomixUseLearning(enabled: boolean) {
	await setPlayerAutomixEnabled(
		get(automixEnabled),
		get(crossfadeMs),
		get(automixDiscoverNew),
		enabled,
		get(automixAllowExternal)
	);
}

export async function setPlayerAutomixAllowExternal(enabled: boolean) {
	await setPlayerAutomixEnabled(
		get(automixEnabled),
		get(crossfadeMs),
		get(automixDiscoverNew),
		get(automixUseLearning),
		enabled
	);
}

export async function togglePlayerAutomix() {
	await setPlayerAutomixEnabled(!get(automixEnabled));
}

export async function cyclePlayerShuffleMode() {
	const currentMode = get(shuffleMode);
	const currentIndex = SHUFFLE_SEQUENCE.indexOf(currentMode);
	const nextMode = SHUFFLE_SEQUENCE[(currentIndex + 1) % SHUFFLE_SEQUENCE.length];
	await setPlayerShuffleMode(nextMode);
}

// Cheap action: stays optimistic — no `assertOnline` gate, no retry button.
export async function addTrackToQueue(trackId: number) {
	try {
		const result = await api.addQueueTrack(trackId);
		setPlaybackQueue(result.queue);
		playerError.set(null);
		noteSuccess();
		showToast('Added to queue', 'success');
		announceQueue('Added to queue');
	} catch (error) {
		setError('add to queue', error);
		throw error;
	}
}

/**
 * Compute the `new_pos` argument for `api.moveQueueTrack` so the row at
 * `targetIndex` lands immediately after the currently-playing row. Exported
 * for unit testing. `new_pos` is measured AFTER the moving row has been
 * removed from the queue, matching moveQueueItem's contract.
 *
 * Returns `null` when the move is a no-op (target is already in place, the
 * queue is too short, or targetIndex is out of bounds).
 */
export function computePlayNextPos(
	targetIndex: number,
	currentIndex: number,
	queueLength: number
): number | null {
	if (queueLength <= 1) return null;
	if (targetIndex < 0 || targetIndex >= queueLength) return null;
	let newPos = currentIndex >= 0 ? currentIndex + 1 : 0;
	if (targetIndex < newPos) newPos -= 1;
	if (newPos === targetIndex) return null;
	return newPos;
}

/**
 * Translate a drag drop-target index into the index `moveQueueItem` (and the
 * server's `move_queue_item`) expect, which is measured AFTER the dragged row is
 * spliced out. Dragging downward (source sits above the target) shifts the
 * target up one slot once the source is removed, so subtract one to land ON the
 * target row's slot (the "drops here" top-edge indicator) instead of one below
 * it. Upward drags keep the target's index. Exported for unit testing.
 */
export function reorderDropIndex(sourceIndex: number, targetIndex: number): number {
	return sourceIndex !== -1 && sourceIndex < targetIndex ? targetIndex - 1 : targetIndex;
}

/**
 * Pick the row that `addQueueTrack` just appended. The endpoint appends exactly
 * one new row; identify it by "id not present before the add" rather than a
 * track-id match, so a track that was already queued doesn't make us grab its
 * pre-existing earlier copy and strand the freshly-added row at the bottom.
 * Falls back to a track-id match when the diff is empty (e.g. server-side
 * dedupe). Exported for unit testing.
 */
export function selectAppendedQueueRow<T extends { id: number; track: { id: number } }>(
	before: T[],
	after: T[],
	trackId: number
): T | undefined {
	const beforeIds = new Set(before.map((item) => item.id));
	return (
		after.find((item) => !beforeIds.has(item.id)) ??
		after.find((item) => item.track.id === trackId)
	);
}

export async function moveQueueTrackNext(queueItemId: number) {
	playerError.set(null);
	const queue = get(playbackQueue);
	const targetIndex = queue.findIndex((item) => item.id === queueItemId);
	if (targetIndex === -1) return;

	// Use the stable item-id move endpoint so duplicate and pending rows remain intact.
	//
	// Anchor on the canonical queue-item anchor (queue-item-id first, track-id
	// fallback), the same helper the active-row highlight and upcomingQueue use.
	// A bare findIndex(track.id) lands on the FIRST duplicate copy of the current
	// track, so "Play next" would compute its target relative to the wrong row.
	const anchor = currentQueueAnchorItem(queue, get(currentTrack), get(currentQueueItemId));
	const currentIndex = anchor ? queue.findIndex((item) => item.id === anchor.id) : -1;
	const newPos = computePlayNextPos(targetIndex, currentIndex, queue.length);
	if (newPos === null) return;

	try {
		const result = await api.moveQueueTrack(queueItemId, newPos);
		setPlaybackQueue(result.queue);
		if (result.playback_state) applyState(result.playback_state);
		noteSuccess();
	} catch (error) {
		setError('reorder queue', error, () => moveQueueTrackNext(queueItemId));
	}
}

export async function removeTrackFromQueue(queueItemId: number) {
	playerError.set(null);
	try {
		const result = await api.removeQueueTrack(queueItemId);
		setPlaybackQueue(result.queue);
		if (result.playback_state) applyState(result.playback_state);
		noteSuccess();
		announceQueue('Removed from queue');
	} catch (error) {
		setError('remove from queue', error, () => removeTrackFromQueue(queueItemId));
	}
}

/// Optimistically reorder a queue item to a new index, then reconcile with the
/// server. If the server returns a different ordering (e.g. another tab moved
/// rows in between), the server response wins.
export async function moveQueueItem(itemId: number, newPos: number) {
	const before = get(playbackQueue);
	const fromIdx = before.findIndex((item) => item.id === itemId);
	if (fromIdx === -1) return;
	const optimistic = before.slice();
	const [moved] = optimistic.splice(fromIdx, 1);
	const target = Math.max(0, Math.min(newPos, optimistic.length));
	optimistic.splice(target, 0, moved);
	setPlaybackQueue(optimistic);

	try {
		const result = await api.moveQueueTrack(itemId, target);
		setPlaybackQueue(result.queue);
		if (result.playback_state) applyState(result.playback_state);
		playerError.set(null);
	} catch (error) {
		// Roll back on failure.
		setPlaybackQueue(before);
		setError('reorder queue', error);
	}
}

export async function clearQueue(): Promise<QueueItem[]> {
	const before = get(playbackQueue);
	try {
		const result = await api.clearQueue();
		setPlaybackQueue(result.queue);
		if (result.playback_state) applyState(result.playback_state);
		playerError.set(null);
		// Offer undo via toast.
		const restorable = before.filter(
			(item) => !result.queue.some((q) => q.id === item.id)
		);
		if (restorable.length > 0) {
			offerUndo(restorable, 6000);
			showToast(`Queue cleared (${restorable.length})`, 'info', 3000);
			announceQueue(`Queue cleared, ${restorable.length} ${restorable.length === 1 ? 'track' : 'tracks'} removed. Press Z to undo.`);
		}
		return restorable;
	} catch (error) {
		setError('clear queue', error);
		return [];
	}
}

/**
 * Restore queue rows after a clear-queue. Re-adds each row in its original
 * order, dispatching by row type so a mixed library + TIDAL + pending queue
 * is restored correctly:
 *  - Library rows (`track.id > 0`) re-add via `api.addQueueTrack`.
 *  - TIDAL-backed pending rows re-add via `api.queueAppend(tidalQueueRequest(...))`.
 *  - Pending rows (`track.id === 0`, never resolved) are skipped because the
 *    library has no `track_id` to re-append; the pending producer would have
 *    to be re-run, which is out of scope here.
 *
 * Returns counts so callers can announce a meaningful summary. Rows that
 * failed to restore are surfaced as a count, not silently dropped.
 */
export interface RestoreSummary {
	restored: number;
	skipped: number;
}

export async function restoreQueueItems(items: QueueItem[]): Promise<RestoreSummary> {
	const summary: RestoreSummary = { restored: 0, skipped: 0 };
	if (!items.length) return summary;
	try {
		for (const item of items) {
			if (item.track.id > 0) {
				await api.addQueueTrack(item.track.id);
				summary.restored += 1;
				continue;
			}
			const tidal = queueItemToTidalPlayable(item);
			if (tidal && tidal.tidal_id > 0) {
				await api.queueAppend(tidalQueueRequest(tidal));
				summary.restored += 1;
				continue;
			}
			// Pending rows (track.id === 0, is_pending) or rows we can't
			// reconstruct (no positive library id, no tidal_id) get skipped.
			summary.skipped += 1;
		}
		const snapshot = await api.getPlaybackState();
		setPlaybackQueue(snapshot.queue);
		playerError.set(null);
		if (summary.restored > 0) {
			announceQueue(
				summary.skipped === 0
					? `Restored ${summary.restored} ${summary.restored === 1 ? 'track' : 'tracks'}`
					: `Restored ${summary.restored}, skipped ${summary.skipped} unresolved`
			);
		}
	} catch (error) {
		setError('restore queue', error);
	}
	return summary;
}

export async function saveQueueAsPlaylist(
	name: string,
	options?: { includeTidalOnly?: boolean }
): Promise<{ id: number; name: string } | null> {
	const trimmed = name.trim();
	if (!trimmed) {
		showToast('Playlist name cannot be empty', 'error');
		return null;
	}
	try {
		const result = await api.createPlaylistFromQueue(
			trimmed,
			options?.includeTidalOnly ?? true
		);
		showToast(`Saved "${result.playlist.name}" — ${result.added} tracks`, 'success');
		playerError.set(null);
		return result.playlist;
	} catch (error) {
		setError('save queue as playlist', error);
		showToast('Failed to save playlist', 'error');
		return null;
	}
}

export function setTrackFavoriteStatus(trackId: number, favorite: boolean, track?: Track) {
	if (track?.tidal_id) {
		if (trackId > 0) {
			tidalFavoriteOverrideById.set(track.tidal_id, { localId: trackId, favorite });
		}
		const previous = tidalMetadataById.get(track.tidal_id) ?? {};
		tidalMetadataById.set(track.tidal_id, {
			...previous,
			track_id: track.id > 0 ? track.id : previous.track_id,
			local_id: track.id > 0 ? track.id : previous.local_id,
			is_in_library: track.id > 0 ? true : previous.is_in_library,
			is_favorite: favorite,
		});
	}
	currentTrack.update((t) =>
		t && t.id === trackId ? { ...t, is_favorite: favorite } : t
	);
	playbackQueue.update((queue) =>
		queue.map((item) =>
			item.track.id === trackId
				? { ...item, track: { ...item.track, is_favorite: favorite } }
				: item
		)
	);
	updateLibraryTrackFavorite(trackId, favorite, track);
}

// Cheap action: stays optimistic — no `assertOnline` gate.
export async function toggleTrackFavorite(trackId: number, currentIsFavorite?: boolean) {
	const current = get(currentTrack);
	const queued = get(playbackQueue).find((item) => item.track.id === trackId)?.track ?? null;
	const playerTrack = current?.id === trackId ? current : queued;

	let nextFavorite: boolean;
	if (currentIsFavorite !== undefined) {
		nextFavorite = !currentIsFavorite;
	} else {
		if (!playerTrack) return;
		nextFavorite = !playerTrack.is_favorite;
	}

	// Optimistic flip — UI updates immediately, rollback on rejection.
	setTrackFavoriteStatus(trackId, nextFavorite, playerTrack ?? undefined);
	try {
		await api.setTrackFavorite(trackId, nextFavorite);
		playerError.set(null);
		noteSuccess();
	} catch (error) {
		// Roll back the optimistic update.
		setTrackFavoriteStatus(trackId, !nextFavorite, playerTrack ?? undefined);
		setError(nextFavorite ? 'like that track' : 'unlike that track', error);
		throw error;
	}
}

// Favourite a TIDAL track that may not be in the library yet. External tracks
// have no local DB row, so — exactly like song radio and download — we import
// on demand to mint a local id, then favourite that. Returns the resolved local
// id and the new favourite state so the calling row/menu can update its
// optimistic UI, or null on failure (the caller rolls back).
export async function toggleTidalTrackFavorite(
	track: TidalPlayable,
	currentIsFavorite = false,
): Promise<{ local_id: number; is_favorite: boolean } | null> {
	const nextFavorite = !currentIsFavorite;
	const existing = track.track_id ?? track.local_id ?? null;
	try {
		let localId = typeof existing === 'number' && existing > 0 ? existing : null;
		if (localId == null) {
			const imported = await api.importTidalTrackForRadio(track);
			localId = imported.local_id;
		}
		await api.setTrackFavorite(localId, nextFavorite);
		// Keep the tidal caches in sync so any surface that re-derives favourite
		// state from tidal_id (now-playing, queue enrichment) sees the change.
		if (track.tidal_id) {
			tidalFavoriteOverrideById.set(track.tidal_id, { localId, favorite: nextFavorite });
			const previous = tidalMetadataById.get(track.tidal_id) ?? {};
			tidalMetadataById.set(track.tidal_id, {
				...previous,
				track_id: localId,
				local_id: localId,
				is_in_library: true,
				is_favorite: nextFavorite,
			});
		}
		playerError.set(null);
		noteSuccess();
		return { local_id: localId, is_favorite: nextFavorite };
	} catch (error) {
		setError(nextFavorite ? 'like that track' : 'unlike that track', error);
		return null;
	}
}

// ─── "Start from here" actions ────────────────────────────────────────────────
// Shared helper: replace the queue with the given track IDs and begin playback
// at the first one. Order matters — the first ID in `trackIds` is played first.
//
// `reasons` is index-aligned with `trackIds`. Radio paths pass per-row
// provenance strings; non-radio callers pass nothing (the queue rows
// land with NULL reasons). The backend persists whatever it gets and
// the queue tooltip reads from the round-tripped QueueItem.reason.
async function loadQueueAndPlay(
	trackIds: number[],
	options?: {
		preserveRadioReasons?: boolean;
		reasons?: (string | null)[];
		pendingCandidates?: PendingCandidateInfo[];
		shuffleMode?: PlaybackState['shuffle_mode'];
		intentSeq?: number;
	},
) {
	if (trackIds.length === 0) return;
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = options?.intentSeq ?? beginPlaybackIntent();
	const ownsIntent = options?.intentSeq == null;
	if (!options?.preserveRadioReasons) clearRadioReasons();
	try {
		const items = [
			...trackIds.map((trackId, index) =>
				libraryTrackToMixedQueueItem(trackId, options?.reasons?.[index])
			),
			...(options?.pendingCandidates ?? []).map((candidate) => ({
				artist: candidate.artist,
				title: candidate.title,
				duration_ms: candidate.duration_ms ?? null,
				reason: candidate.reason ?? null,
			})),
		];
		const replaced = await api.replacePlaybackQueue(items, {
			shuffleMode: options?.shuffleMode,
			startPlayback: true,
		});
		if (!isLatestPlaybackIntent(intentSeq)) return;
		hydratePlaybackIfLatest({ state: replaced.state, queue: replaced.queue }, intentSeq);
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('start playback', error, () => loadQueueAndPlay(trackIds, options));
	} finally {
		if (ownsIntent) finishPlaybackIntent(intentSeq);
	}
}

// Slice a track-id list so the queue starts at `startTrackId` and runs to the
// end of the list, dropping the tracks before it. This is what "click a row"
// means in TIDAL/Spotify: the rows AFTER the one you clicked are what play next,
// not the rows above it. Without the slice, clicking row 15 of a list would
// replay rows 1-14 right after it. This helper receives library ids only;`r`n// TIDAL-only rows use canonical mixed queue inputs. When no start
// track is given (a bare "Play all") the whole list is returned in order. Pure +
// exported so the slice contract is unit tested without a live server.
export function sliceContextTrackIds(trackIds: number[], startTrackId?: number): number[] {
	const ids = trackIds.filter((id) => id > 0);
	if (startTrackId == null) return ids;
	const idx = ids.indexOf(startTrackId);
	if (idx <= 0) return ids;
	return ids.slice(idx);
}

/**
 * Canonical "play in context" action. Treats `trackIds` as the list the user is
 * looking at and makes it the queue, beginning at `startTrackId` (or the first
 * track). Clicking a row in any track list should call this — the list you see
 * becomes the queue, mirroring TIDAL/Spotify, instead of playing one orphan
 * track and letting automix improvise the rest.
 *
 * Reordering the start track to the front (rather than tracking a play cursor)
 * matches the established playAlbum/playArtist/playPlaylist convention so every
 * surface behaves identically. When starting from a specific track shuffle is
 * forced off; a bare "Play all" honors the user's global shuffle mode.
 */
export async function playTracksInContext(
	trackIds: number[],
	startTrackId?: number,
	options?: { shuffle?: boolean },
) {
	const ids = trackIds.filter((id) => id > 0);
	if (ids.length === 0) return;
	if (options?.shuffle) {
		await loadQueueAndPlay(ids, { shuffleMode: 'true' });
		return;
	}
	await loadQueueAndPlay(sliceContextTrackIds(ids, startTrackId), {
		shuffleMode: startTrackId != null ? undefined : get(shuffleMode),
	});
}

// The catalog list endpoint caps a single page at 200 rows; that is a sensible
// queue depth for "Play"/"Shuffle" on the whole library since automix extends
// from there. Pulling every id (tens of thousands) into one POST is neither
// necessary nor cheap. For Shuffle the server draws those 200 with ORDER BY
// RANDOM() so the sample is a fresh random slice of the WHOLE library, not the
// newest-200 prefix reshuffled in place.
const LIBRARY_QUEUE_LIMIT = 200;

/**
 * Play the user's library as a queue. Fetches up to `LIBRARY_QUEUE_LIMIT` tracks
 * in the requested sort / liked context and loads them, mirroring the Play (or
 * Shuffle) button on a TIDAL/Spotify collection. Without this, the library had
 * no way to start a real session: clicking a track only played that one track.
 */
export async function playLibrary(options?: {
	sortBy?: string;
	sortDir?: string;
	likedOnly?: boolean;
	shuffle?: boolean;
}) {
	if (!assertOnline()) return;
	playerError.set(null);
	try {
		// Shuffle pulls a random slice of the entire library; Play honors the
		// active sort. Without 'random' here, Shuffle only ever saw the first 200
		// rows of the current sort (e.g. the 200 newest) and reshuffled those.
		const { tracks } = await api.getTracks(
			options?.shuffle ? 'random' : (options?.sortBy ?? 'date_added'),
			options?.sortDir ?? 'desc',
			LIBRARY_QUEUE_LIMIT,
			0,
			true,
			options?.likedOnly ?? false,
		);
		if (tracks.length === 0) {
			playerError.set({ message: 'No tracks in your library yet.' });
			return;
		}
		await playTracksInContext(
			tracks.map((t) => t.id),
			undefined,
			{ shuffle: options?.shuffle },
		);
	} catch (error) {
		setError('play your library', error, () => playLibrary(options));
	}
}

export function selectOptimisticNextItem<T extends { id: number; track: { id: number } }>(
	queue: T[],
	currentTrackId: number | null | undefined,
	currentQueueItem: number | null | undefined
): T | null {
	let currentIdx =
		currentQueueItem != null ? queue.findIndex((q) => q.id === currentQueueItem) : -1;
	if (currentIdx < 0) {
		currentIdx = queue.findIndex((q) => q.track.id === (currentTrackId ?? -1));
	}
	return currentIdx >= 0 && currentIdx + 1 < queue.length ? queue[currentIdx + 1] : null;
}

/**
 * The album-tracks payload a page already holds. Passing it into
 * playAlbum/shuffleAlbum skips the refetch (which, for partial albums, is a
 * live TIDAL round trip on the server) and guarantees the queue matches the
 * listing the user is looking at.
 */
export interface AlbumTracksData {
	tracks: Track[];
	tidal_tracks?: TidalDiscographyTrack[] | null;
	album_tidal_id?: number | null;
}

export async function playAlbum(
	albumId: number,
	startTrackId?: number,
	preloaded?: AlbumTracksData,
) {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const data = preloaded ?? (await api.getAlbumTracks(albumId));
		if (!isLatestPlaybackIntent(intentSeq)) return;
		const tracks = data.tracks;
		const tidalOnly = data.tidal_tracks ?? [];

		// Partial album: some tracks live only on TIDAL (not imported). Queue the
		// WHOLE album in (disc, track) order through the mixed pending-queue
		// pipeline: owned rows play from the library (bit-perfect local path,
		// including local-only rips with no TIDAL id) and TIDAL-only rows resolve
		// lazily at play time - skipped, not fatal, when TIDAL is unavailable.
		// Queueing only the owned subset left a short queue that automix padded
		// with unrelated "similar" tracks, so the album appeared to play a
		// random set of songs instead of the album.
		if (tidalOnly.length > 0) {
			clearRadioReasons();
			const entries = mergeAlbumTracks(tracks, tidalOnly);
			const ordered = entries.slice(albumEntryStartIndex(entries, startTrackId));
			const result = await api.replacePlaybackQueue(
				ordered.map(albumEntryToMixedQueueItem),
				{ shuffleMode: startTrackId == null ? get(shuffleMode) : undefined, startPlayback: true }
			);
			if (!isLatestPlaybackIntent(intentSeq)) return;
			hydratePlaybackIfLatest({ state: result.state, queue: result.queue }, intentSeq);
			noteSuccess();
			const streamed = ordered.filter((e) => e.kind === 'tidal').length;
			showToast(`Playing album (${ordered.length} tracks, ${streamed} streamed)`, 'success');
			return;
		}

		// Fully-owned or local-only album: the owned rows already are the whole
		// album, so keep the plain local queue path.
		if (tracks.length === 0) {
			playerError.set({ message: 'Album has no tracks.' });
			return;
		}
		await loadQueueAndPlay(sliceContextTrackIds(tracks.map((t) => t.id), startTrackId), {
			shuffleMode: startTrackId ? undefined : get(shuffleMode),
			intentSeq,
		});
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('play that album', error, () => playAlbum(albumId, startTrackId, preloaded));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function shuffleAlbum(albumId: number, preloaded?: AlbumTracksData) {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const data = preloaded ?? (await api.getAlbumTracks(albumId));
		if (!isLatestPlaybackIntent(intentSeq)) return;
		const tracks = data.tracks;
		const tidalOnly = data.tidal_tracks ?? [];

		// Partial album: shuffle the WHOLE album (owned + TIDAL-only rows), the
		// same mixed-queue path as playAlbum. Shuffling only the owned subset
		// left a short queue that automix padded with unrelated tracks.
		if (tidalOnly.length > 0) {
			clearRadioReasons();
			const entries = mergeAlbumTracks(tracks, tidalOnly);
			const result = await api.replacePlaybackQueue(entries.map(albumEntryToMixedQueueItem), {
				shuffleMode: 'true',
				startPlayback: true,
			});
			if (!isLatestPlaybackIntent(intentSeq)) return;
			hydratePlaybackIfLatest({ state: result.state, queue: result.queue }, intentSeq);
			noteSuccess();
			showToast(`Shuffling album (${entries.length} tracks)`, 'success');
			return;
		}

		if (tracks.length === 0) {
			playerError.set({ message: 'Album has no tracks.' });
			return;
		}
		await loadQueueAndPlay(tracks.map((t) => t.id), { shuffleMode: 'true', intentSeq });
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('shuffle that album', error, () => shuffleAlbum(albumId, preloaded));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

/**
 * Save a TIDAL album into the local library by importing every track, so it
 * becomes a real library album. Matches Spotify/TIDAL "Save to library".
 * Returns the resulting local album id, or null on failure.
 */
export async function saveTidalAlbumToLibrary(tidalAlbumId: number): Promise<number | null> {
	if (!assertOnline()) return null;
	try {
		const res = await api.importTidalAlbum(tidalAlbumId);
		showToast('Added to library', 'success');
		return res.album_id;
	} catch (error) {
		setError('save that album', error, () =>
			saveTidalAlbumToLibrary(tidalAlbumId).then(() => {}),
		);
		return null;
	}
}

/**
 * Toggle an album's liked state (local favorite flag + TIDAL favorite sync).
 * `currentIsFavorite` is the state before the click. Returns the new state, or
 * the unchanged state on failure so the caller can roll its optimistic flip back.
 */
export async function toggleAlbumFavorite(
	albumId: number,
	currentIsFavorite: boolean,
): Promise<boolean> {
	const next = !currentIsFavorite;
	try {
		await api.setAlbumFavorite(albumId, next);
		return next;
	} catch (error) {
		setError(next ? 'like that album' : 'unlike that album', error);
		return currentIsFavorite;
	}
}

export async function playArtist(artistId: number, startTrackId?: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const { tracks } = await api.getArtistTracks(artistId);
		if (!isLatestPlaybackIntent(intentSeq)) return;
		if (tracks.length === 0) {
			playerError.set({ message: 'Artist has no tracks.' });
			return;
		}
		await loadQueueAndPlay(sliceContextTrackIds(tracks.map((t) => t.id), startTrackId), {
			shuffleMode: startTrackId ? undefined : get(shuffleMode),
			intentSeq,
		});
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('play that artist', error, () => playArtist(artistId, startTrackId));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function shuffleArtist(artistId: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const { tracks } = await api.getArtistTracks(artistId);
		if (!isLatestPlaybackIntent(intentSeq)) return;
		if (tracks.length === 0) {
			playerError.set({ message: 'Artist has no tracks.' });
			return;
		}
		await loadQueueAndPlay(tracks.map((t) => t.id), { shuffleMode: 'true', intentSeq });
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('shuffle that artist', error, () => shuffleArtist(artistId));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

async function startSongRadioFromLibraryTrack(
	seedTrackId: number,
	intentSeq: number
): Promise<boolean> {
	const result = await api.startRadioStart({ seed_track_id: seedTrackId, limit: 60 });
	if (!isLatestPlaybackIntent(intentSeq)) return false;
	hydratePlayback({ state: result.state, queue: result.queue });
	return true;
}

export async function startSongRadio(seedTrackId: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	// Server-side fallback (artist.getsimilar when track-level recall is empty)
	// can take a few seconds. Surface a loading toast so the user knows the
	// click registered. Dismissed before the success/error toast lands.
	const loadingToastId = showToast('Starting Song Radio...', 'info', 8000);
	try {
		const applied = await startSongRadioFromLibraryTrack(seedTrackId, intentSeq);

		dismissToast(loadingToastId);
		if (!applied) return;
		showToast('Song Radio started', 'success');
	} catch (error) {
		dismissToast(loadingToastId);
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('start radio', error, () => startSongRadio(seedTrackId));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function shufflePlaylist(
	tracks: { id: number }[],
) {
	if (!tracks.length) return;
	await loadQueueAndPlay(tracks.map((t) => t.id), { shuffleMode: 'true' });
	showToast('Shuffling playlist', 'success');
}

/**
 * Play an entire playlist in order, optionally jumping to a specific track
 * first. Mirrors the `playAlbum` shape so callers can swap one for the other.
 * Without this helper the desktop pattern only plays `tracks[0]` and relies on
 * automix to drag the rest along, which doesn't match user expectation when
 * they tap Play on a playlist.
 */
export async function playPlaylist(playlistId: number, startTrackId?: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const { tracks } = await api.getPlaylistTracks(playlistId);
		if (!isLatestPlaybackIntent(intentSeq)) return;
		if (tracks.length === 0) {
			playerError.set({ message: 'Playlist is empty.' });
			return;
		}
		await loadQueueAndPlay(sliceContextTrackIds(tracks.map((t) => t.id), startTrackId), {
			shuffleMode: startTrackId ? undefined : get(shuffleMode),
			intentSeq,
		});
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('play that playlist', error, () => playPlaylist(playlistId, startTrackId));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function startPlaylistRadio(tracks: { id: number; play_count?: number }[]) {
	if (!tracks.length) return;
	// Seed from the most-played track; fall back to first track
	const seed = [...tracks].sort((a, b) => (b.play_count ?? 0) - (a.play_count ?? 0))[0];
	await startSongRadio(seed.id);
}

export async function playTidalPlaylist(tidalUuid: string) {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const { tracks } = await api.getTidalPlaylistTracks(tidalUuid);
		if (!isLatestPlaybackIntent(intentSeq)) return;
		if (!tracks.length) {
			playerError.set({ message: 'No playable tracks in this playlist.' });
			return;
		}
		await startTidalQueue(tracks, { shuffleMode: get(shuffleMode), intentSeq });
		if (!isLatestPlaybackIntent(intentSeq)) return;
		showToast(`Playing playlist (${tracks.length} tracks queued)`, 'success');
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('load TIDAL playlist', error, () => playTidalPlaylist(tidalUuid));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function startArtistRadio(artistId: number, _seedTrackId?: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const queue = await api.startRadioArtist({ seed_artist_id: artistId, limit: 60 });
		if (!isLatestPlaybackIntent(intentSeq)) return;
		if (!queue.first_playable) {
			playerError.set({ message: 'No tracks found for radio.' });
			return;
		}
		setRadioReasons(queue.tracks);
		if (queue.state && queue.queue) {
			hydratePlayback({ state: queue.state, queue: queue.queue });
		}
		showToast(`Radio from ${queue.seed.title}`, 'success');
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('start artist radio', error, () => startArtistRadio(artistId, _seedTrackId));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function startAlbumRadio(albumId: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const queue = await api.startRadioAlbum({ seed_album_id: albumId, limit: 60 });
		if (!isLatestPlaybackIntent(intentSeq)) return;
		if (!queue.first_playable) {
			playerError.set({ message: 'No tracks found for radio.' });
			return;
		}
		setRadioReasons(queue.tracks);
		if (queue.state && queue.queue) {
			hydratePlayback({ state: queue.state, queue: queue.queue });
		}
		showToast(`Radio from ${queue.seed.title}`, 'success');
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('start album radio', error, () => startAlbumRadio(albumId));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

/**
 * Genre-flavored radio: seed the real radio orchestrator from a representative
 * track of a genre / mood / heat set. Unlike a static replacePlaybackQueue of
 * raw genre rows (which can strand on unplayable TIDAL-only tracks and loop the
 * one playable seed), this builds a continuous, reasoned station and hydrates
 * the player like Song/Artist radio. `label` is user-facing ("Vibe", "Hottest").
 */
export async function startGenreRadio(seedTrackId: number, blend: RadioBlend, label: string) {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	const loadingToastId = showToast(`Starting ${label} radio...`, 'info', 8000);
	try {
		const queue = await api.startRadioSong({ seed_track_id: seedTrackId, blend, limit: 60 });
		dismissToast(loadingToastId);
		if (!isLatestPlaybackIntent(intentSeq)) return;
		if (!queue.first_playable) {
			playerError.set({ message: 'No radio tracks found for that seed.' });
			return;
		}
		setRadioReasons(queue.tracks);
		if (queue.state && queue.queue) {
			hydratePlayback({ state: queue.state, queue: queue.queue });
		}
		showToast(`${label} radio started`, 'success');
	} catch (error) {
		dismissToast(loadingToastId);
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('start radio', error, () => startGenreRadio(seedTrackId, blend, label));
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function playTrackNext(trackId: number) {
	playerError.set(null);
	// Add to queue, then move next to the currently-playing track.
	try {
		const before = get(playbackQueue);
		const addResult = await api.addQueueTrack(trackId);
		setPlaybackQueue(addResult.queue);
		// Pick the genuinely-new row (id not present before the add), not the first
		// track-id match: if the track was already queued, a track-id match returns
		// the pre-existing earlier copy and the freshly appended row is stranded at
		// the bottom (the "Play next went to the bottom" bug).
		const justAdded = selectAppendedQueueRow(before, addResult.queue, trackId);
		if (justAdded) {
			await moveQueueTrackNext(justAdded.id);
		}
		noteSuccess();
	} catch (error) {
		setError('queue that track', error, () => playTrackNext(trackId));
	}
}

export async function playTidalTrackNow(track: TidalPlayable): Promise<void> {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		rememberTidalPlayable(track);
		setOptimisticTidalTrack(track);
		const result = await api.replacePlaybackQueue([tidalPlayableToMixedQueueItem(track)], {
			startPlayback: true,
		});
		if (!isLatestPlaybackIntent(intentSeq)) return;
		hydratePlaybackIfLatest({ state: result.state, queue: result.queue }, intentSeq);
		noteSuccess();
		showToast(`Playing ${trackLabel(track)}`, 'success');
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('play that Tidal track', error, () => playTidalTrackNow(track));
		showToast(`Playback failed`, 'error');
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

function tidalQueueRequest(track: TidalPlayable) {
	rememberTidalPlayable(track);
	return {
		kind: 'tidal' as const,
		tidal_id: track.tidal_id,
		artist: track.artist_name ?? 'Unknown Artist',
		title: track.title,
		album_title: track.album_title,
		artwork_url: track.artwork_url ?? null,
		artist_tidal_id: track.artist_tidal_id ?? null,
		album_tidal_id: track.album_tidal_id ?? null,
		duration_ms: track.duration_ms
	};
}

function setOptimisticTidalTrack(track: TidalPlayable) {
	rememberTidalPlayable(track);
	resetOptimisticPlaybackProgress();
	setCurrentTrack({
		id: localTidalTrackId(track) ?? -track.tidal_id,
		title: track.title,
		artist_id: -1,
		artist_name: track.artist_name,
		artist_tidal_id: track.artist_tidal_id ?? null,
		album_id: null,
		album_title: track.album_title,
		album_tidal_id: track.album_tidal_id ?? null,
		disc_number: null,
		track_number: null,
		duration_ms: track.duration_ms,
		isrc: null,
		tidal_id: track.tidal_id,
		best_quality: 'LOSSLESS',
		best_source: 'tidal',
		fidelity_score: 0,
		is_favorite: track.is_favorite ?? false,
		play_count: 0,
		last_played_at: null,
		date_added: null,
		source: 'tidal_stream',
		artwork_url: track.artwork_url,
	});
	isPlaying.set(true);
}

export async function playTidalTrackNext(track: TidalPlayable): Promise<void> {
	playerError.set(null);
	try {
		const result = await api.queuePlayNext(tidalQueueRequest(track));
		setPlaybackQueue(result.queue);
		noteSuccess();
		showToast(`Queued next: ${trackLabel(track)}`, 'success');
	} catch (error) {
		setError('queue that Tidal track next', error, () => playTidalTrackNext(track));
	}
}

export async function playTidalTracksNow(
	tracks: TidalPlayable[],
	label = 'playlist',
	options?: { shuffleMode?: PlaybackState['shuffle_mode']; startIndex?: number }
): Promise<void> {
	if (!assertOnline()) return;
	// Mirror sliceContextTrackIds: clicking row N makes "the list from N" the
	// queue (no wrap), matching the library album/playlist convention. A bare
	// "Play all" passes no startIndex and plays the whole list. Starting from a
	// specific row forces shuffle off, like playAlbum does with startTrackId.
	const startIndex = options?.startIndex ?? 0;
	const ordered = startIndex > 0 ? tracks.slice(startIndex) : tracks;
	const playable = ordered.filter((track) => track.tidal_id > 0);
	if (!playable.length) {
		showToast('No playable tracks ready yet', 'info');
		return;
	}
	rememberTidalPlayables(playable);
	playerError.set(null);
	clearRadioReasons();
	const intentSeq = beginPlaybackIntent();
	try {
		const requestShuffleMode =
			options?.shuffleMode ?? (startIndex > 0 ? 'off' : get(shuffleMode));
		const oneShotShuffleMode = requestShuffleMode === 'off' ? undefined : requestShuffleMode;
		if (!oneShotShuffleMode && isLatestPlaybackIntent(intentSeq)) {
			setOptimisticTidalTrack(playable[0]);
		}
		// Unified queue: library-known rows ride as library tracks, the rest as
		// metadata-rich pending rows resolved by the import pipeline.
		const result = await api.replacePlaybackQueue(
			playable.map(tidalPlayableToMixedQueueItem),
			{ shuffleMode: oneShotShuffleMode, startPlayback: true }
		);
		if (!isLatestPlaybackIntent(intentSeq)) return;
		hydratePlaybackIfLatest({ state: result.state, queue: result.queue }, intentSeq);
		noteSuccess();
		showToast(`Playing ${label} (${playable.length} tracks)`, 'success');
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('play those Tidal tracks', error, () => playTidalTracksNow(tracks, label, options));
		showToast('Playback failed', 'error');
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function shuffleTidalTracksNow(tracks: TidalPlayable[], label = 'playlist'): Promise<void> {
	await playTidalTracksNow(tracks, label, { shuffleMode: 'true' });
}

export async function playTidalTracksNext(tracks: TidalPlayable[]): Promise<void> {
	const playable = tracks.filter((track) => track.tidal_id > 0);
	if (!playable.length) {
		showToast('No playable tracks ready yet', 'info');
		return;
	}
	playerError.set(null);
	try {
		const result = await api.queuePlayNextMany(playable.map(tidalQueueRequest));
		setPlaybackQueue(result.queue);
		noteSuccess();
		showToast(`Queued next: ${playable.length} tracks`, 'success');
	} catch (error) {
		setError('queue those Tidal tracks next', error, () => playTidalTracksNext(tracks));
	}
}

export async function addTidalTrackToQueue(track: TidalPlayable): Promise<void> {
	playerError.set(null);
	try {
		const result = await api.queueAppend(tidalQueueRequest(track));
		setPlaybackQueue(result.queue);
		noteSuccess();
		showToast(`Added to queue: ${trackLabel(track)}`, 'success');
	} catch (error) {
		setError('add that Tidal track to queue', error, () => addTidalTrackToQueue(track));
	}
}

export async function addTidalTracksToQueue(tracks: TidalPlayable[]): Promise<void> {
	const playable = tracks.filter((track) => track.tidal_id > 0);
	if (!playable.length) {
		showToast('No playable tracks ready yet', 'info');
		return;
	}
	playerError.set(null);
	try {
		const result = await api.queueAppendMany(playable.map(tidalQueueRequest));
		setPlaybackQueue(result.queue);
		noteSuccess();
		showToast(`Added to queue: ${playable.length} tracks`, 'success');
	} catch (error) {
		setError('add those Tidal tracks to queue', error, () => addTidalTracksToQueue(tracks));
	}
}

export async function playTidalAlbum(tidalAlbumId: number): Promise<void> {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const { tracks } = await api.getTidalAlbumTracks(tidalAlbumId);
		if (!isLatestPlaybackIntent(intentSeq)) return;
		if (tracks.length === 0) {
			showToast('Album has no tracks', 'error');
			return;
		}
		await startTidalQueue(tracks, { shuffleMode: get(shuffleMode), intentSeq });
		if (!isLatestPlaybackIntent(intentSeq)) return;
		showToast(`Playing album (${tracks.length} tracks queued)`, 'success');
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('play that Tidal album', error, () => playTidalAlbum(tidalAlbumId));
		showToast(`Album playback failed`, 'error');
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

async function startTidalQueue(
	tracks: ReadonlyArray<{
		tidal_id: number;
		title: string;
		artist_name?: string | null;
		album_title?: string | null;
		artwork_url?: string | null;
		duration_ms?: number | null;
		artist_tidal_id?: number | null;
		album_tidal_id?: number | null;
		track_id?: number;
		local_id?: number | null;
		is_in_library?: boolean;
		is_favorite?: boolean;
	}>,
	options?: { shuffleMode?: PlaybackState['shuffle_mode']; intentSeq?: number }
): Promise<void> {
	rememberTidalPlayables(tracks);
	clearRadioReasons();
	const intentSeq = options?.intentSeq ?? beginPlaybackIntent();
	const ownsIntent = options?.intentSeq == null;
	const requestShuffleMode = options?.shuffleMode;
	const oneShotShuffleMode = requestShuffleMode === 'off' ? undefined : requestShuffleMode;
	const playable = tracks.map((t) => ({
		tidal_id: t.tidal_id,
		title: t.title,
		artist_name: t.artist_name ?? null,
		artist_tidal_id: t.artist_tidal_id ?? null,
		album_title: t.album_title ?? null,
		album_tidal_id: t.album_tidal_id ?? null,
		artwork_url: t.artwork_url ?? null,
		duration_ms: t.duration_ms ?? null,
		track_id: t.track_id,
		local_id: t.local_id ?? null,
		is_in_library: t.is_in_library,
		is_favorite: t.is_favorite,
	}));

	try {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		if (!oneShotShuffleMode) setOptimisticTidalTrack(playable[0]);
		// Unified queue: library-known rows ride as library tracks, the rest as
		// metadata-rich pending rows resolved by the import pipeline.
		const result = await api.replacePlaybackQueue(
			playable.map(tidalPlayableToMixedQueueItem),
			{ shuffleMode: oneShotShuffleMode, startPlayback: true }
		);
		if (!isLatestPlaybackIntent(intentSeq)) return;
		hydratePlaybackIfLatest({ state: result.state, queue: result.queue }, intentSeq);
		noteSuccess();
	} finally {
		if (ownsIntent) finishPlaybackIntent(intentSeq);
	}
}

export async function playTidalMix(mixId: string): Promise<void> {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	try {
		const { tracks } = await api.getTidalMixTracks(mixId);
		if (!isLatestPlaybackIntent(intentSeq)) return;
		if (tracks.length === 0) {
			showToast('Mix has no tracks', 'error');
			return;
		}
		await startTidalQueue(tracks, { intentSeq });
		if (!isLatestPlaybackIntent(intentSeq)) return;
		showToast(`Playing mix (${tracks.length} tracks queued)`, 'success');
	} catch (error) {
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('play that Tidal mix', error, () => playTidalMix(mixId));
		showToast(`Mix playback failed`, 'error');
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}

export async function startTidalSongRadio(track: TidalPlayable): Promise<void> {
	if (!assertOnline()) return;
	playerError.set(null);
	const intentSeq = beginPlaybackIntent();
	const loadingToastId = showToast('Starting Song Radio...', 'info', 8000);
	try {
	// Last.fm chart entries that didn't resolve locally arrive with the
	// placeholder `tidal_id: 0` (see ChartTidalPlayable in routes.rs). Resolve
	// to a real Tidal id via search first — otherwise the import fallback
	// below collides on `WHERE tidal_id = 0` and seeds radio from whatever
	// unrelated track happens to already hold that placeholder row.
	if (track.tidal_id <= 0) {
		const q = [track.artist_name, track.title].filter(Boolean).join(' ');
		if (!q) {
			dismissToast(loadingToastId);
			showToast(`Couldn't find "${trackLabel(track)}" on Tidal`, 'info');
			return;
		}
		try {
			const results = await api.searchTidal(q, 1);
			if (!isLatestPlaybackIntent(intentSeq)) {
				dismissToast(loadingToastId);
				return;
			}
			const hit = results.tracks[0];
			if (!hit) {
				dismissToast(loadingToastId);
				showToast(`Couldn't find "${trackLabel(track)}" on Tidal`, 'info');
				return;
			}
			track = {
				tidal_id: hit.tidal_id,
				title: hit.title,
				artist_name: hit.artist_name,
				album_title: hit.album_title,
				artwork_url: hit.artwork_url ?? track.artwork_url,
				duration_ms: hit.duration_ms,
				artist_tidal_id: null,
			};
		} catch (error) {
			dismissToast(loadingToastId);
			if (!isLatestPlaybackIntent(intentSeq)) return;
			setError('start Tidal radio', error, () => startTidalSongRadio(track));
			return;
		}
	}
	// Try discovery radio seeded directly by Tidal ID (only works if track is already in library)
	try {
		const { tracks } = await api.getRadioTracks({ seed_tidal_id: track.tidal_id, limit: 40 });
		if (!isLatestPlaybackIntent(intentSeq)) {
			dismissToast(loadingToastId);
			return;
		}
		const radioIds = tracks.map((t) => t.track_id);
		if (radioIds.length > 0) {
			await loadQueueAndPlay(radioIds, { intentSeq });
			dismissToast(loadingToastId);
			if (!isLatestPlaybackIntent(intentSeq)) return;
			showToast(`Radio from ${trackLabel(track)}`, 'success');
			playerError.set(null);
			return;
		}
	} catch (error) {
		// 404 = track not yet in library index — fall through to silent import.
		// Any other error is a real failure.
		if (!isLatestPlaybackIntent(intentSeq)) {
			dismissToast(loadingToastId);
			return;
		}
		if (!(error instanceof ApiError && error.status === 404)) {
			dismissToast(loadingToastId);
			setError('start Tidal radio', error, () => startTidalSongRadio(track));
			showToast(`Radio failed`, 'error');
			return;
		}
	}

	// Fallback: silently import the track as a tidal_stream entry (invisible in library grids)
	// so the radio engine can use it as a seed, then run song radio from the resulting local ID.
	try {
		const { local_id } = await api.importTidalTrackForRadio(track);
		if (!isLatestPlaybackIntent(intentSeq)) {
			dismissToast(loadingToastId);
			return;
		}
		const applied = await startSongRadioFromLibraryTrack(local_id, intentSeq);
		dismissToast(loadingToastId);
		if (!applied) return;
		showToast(`Radio from ${trackLabel(track)}`, 'success');
		playerError.set(null);
	} catch (error) {
		dismissToast(loadingToastId);
		if (!isLatestPlaybackIntent(intentSeq)) return;
		setError('start Tidal radio', error, () => startTidalSongRadio(track));
		showToast(`No radio results for "${trackLabel(track)}"`, 'info');
	}
	} finally {
		finishPlaybackIntent(intentSeq);
	}
}
