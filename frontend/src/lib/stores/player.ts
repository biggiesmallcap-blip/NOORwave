import { get, writable } from 'svelte/store';
import {
	api,
	ApiError,
	type AudioDspFeatures,
	type PendingCandidateInfo,
	type PlaybackSnapshot,
	type PlaybackState,
	type QueueItem,
	type StreamDisplayInfo,
	type TidalPlayable,
	type Track
} from '$lib/api/client';
import { showToast, dismissToast } from '$lib/stores/toast';
import { wsConnected } from '$lib/api/ws';
import { updateLibraryTrackFavorite } from '$lib/stores/library';

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

currentTrack.subscribe((track) => {
	const nextId = track?.id ?? null;
	if (nextId === _lastFeaturesTrackId) return;
	_lastFeaturesTrackId = nextId;

	if (nextId === null) {
		currentTrackFeatures.set(null);
		return;
	}

	const seq = ++_featuresFetchSeq;
	// Clear stale features immediately so UI doesn't show the previous track's badge.
	currentTrackFeatures.set(null);

	void api
		.getTrackAudioFeatures(nextId)
		.then((res) => {
			// Guard against out-of-order responses.
			if (seq !== _featuresFetchSeq) return;
			currentTrackFeatures.set(res.features ?? null);
		})
		.catch(() => {
			if (seq !== _featuresFetchSeq) return;
			currentTrackFeatures.set(null);
		});
});

export const isPlaying = writable(false);
export const position = writable(0);
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
	currentTrack.set(state.current_track);
	currentQueueItemId.set(state.current_queue_item_id ?? null);
	isPlaying.set(state.is_playing);
	position.set(state.position_ms);
	anchorPositionTicker(state.position_ms);
	volume.set(state.volume);
	shuffleMode.set(state.shuffle_mode);
	repeatMode.set(state.repeat_mode);
	automixEnabled.set(state.automix_enabled);
	crossfadeMs.set(state.crossfade_ms);
	automixDiscoverNew.set(state.automix_discover_new);
	automixUseLearning.set(state.automix_use_learning);
	automixAllowExternal.set(state.automix_allow_external);
}

export function hydratePlayback(snapshot: PlaybackSnapshot) {
	applyState(snapshot.state);
	playbackQueue.set(snapshot.queue);
	playerReady.set(true);
	playerError.set(null);
	noteSuccess();
}

export async function refreshPlaybackState() {
	playerError.set(null);
	try {
		const snapshot = await api.getPlaybackState();
		hydratePlayback(snapshot);
	} catch (error) {
		setError('load playback state', error, () => refreshPlaybackState());
	}
}

export async function playTrackNow(trackId: number) {
	playerError.set(null);
	try {
		const snapshot = await api.playTrack(trackId);
		hydratePlayback(snapshot);
	} catch (error) {
		setError('play that track', error, () => playTrackNow(trackId));
	}
}

export async function togglePlayback() {
	playerError.set(null);
	try {
		if (get(isPlaying)) {
			const result = await api.pausePlayback();
			applyState(result.state);
		} else {
			const result = await api.resumePlayback();
			applyState(result.state);
		}
		noteSuccess();
	} catch (error) {
		setError('toggle playback', error, () => togglePlayback());
	}
}

export async function playPreviousTrack() {
	playerError.set(null);
	try {
		const snapshot = await api.previousTrack();
		hydratePlayback(snapshot);
	} catch (error) {
		setError('go to previous track', error, () => playPreviousTrack());
	}
}

export async function playNextTrack() {
	playerError.set(null);
	try {
		// Optimistic update: show the next queued track immediately rather than waiting
		// for TIDAL stream resolution (~2-5s). hydratePlayback below corrects any mismatch.
		const currentRepeat = get(repeatMode);
		if (currentRepeat !== 'one') {
			const queue = get(playbackQueue);
			const current = get(currentTrack);
			const currentIdx = queue.findIndex((q) => q.track.id === (current?.id ?? -1));
			const nextItem =
				currentIdx >= 0 && currentIdx + 1 < queue.length ? queue[currentIdx + 1] : null;
			if (nextItem) {
				currentTrack.set(nextItem.track);
			}
		}
		position.set(0);
		anchorPositionTicker(0);

		const snapshot = await api.nextTrack();
		hydratePlayback(snapshot);
	} catch (error) {
		setError('skip to next track', error, () => playNextTrack());
	}
}

export async function setPlayerVolume(nextVolume: number) {
	playerError.set(null);
	try {
		const clamped = Math.max(0, Math.min(1, nextVolume));
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
	try {
		const result = await api.setPlaybackPosition(nextPositionMs);
		applyState(result.state);
		noteSuccess();
	} catch (error) {
		setError('seek', error, () => setPlayerPosition(nextPositionMs));
	}
}

export async function setPlayerRepeatMode(mode: PlaybackState['repeat_mode']) {
	playerError.set(null);
	try {
		const result = await api.setPlaybackRepeat(mode);
		applyState(result.state);
		noteSuccess();
	} catch (error) {
		setError('set repeat mode', error, () => setPlayerRepeatMode(mode));
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
	try {
		const snapshot = await api.setPlaybackShuffle(mode);
		hydratePlayback(snapshot);
	} catch (error) {
		setError('set shuffle mode', error, () => setPlayerShuffleMode(mode));
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
		if (result.queue) playbackQueue.set(result.queue);
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
		playbackQueue.set(result.queue);
		playerError.set(null);
		noteSuccess();
		showToast('Added to queue', 'success');
	} catch (error) {
		setError('add to queue', error);
		throw error;
	}
}

export async function moveQueueTrackNext(queueItemId: number) {
	playerError.set(null);
	const queue = [...get(playbackQueue)];
	const targetIndex = queue.findIndex((item) => item.id === queueItemId);
	if (targetIndex === -1 || queue.length <= 1) return;

	const [targetItem] = queue.splice(targetIndex, 1);
	const currentTrackId = get(currentTrack)?.id ?? null;
	const currentIndex = currentTrackId
		? queue.findIndex((item) => item.track.id === currentTrackId)
		: -1;
	const insertIndex = currentIndex >= 0 ? currentIndex + 1 : 0;
	queue.splice(insertIndex, 0, targetItem);

	try {
		const result = await api.replacePlaybackQueue(queue.map((item) => item.track.id));
		playbackQueue.set(result.queue);
		noteSuccess();
	} catch (error) {
		setError('reorder queue', error, () => moveQueueTrackNext(queueItemId));
	}
}

export async function removeTrackFromQueue(queueItemId: number) {
	playerError.set(null);
	try {
		const result = await api.removeQueueTrack(queueItemId);
		playbackQueue.set(result.queue);
		noteSuccess();
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
	playbackQueue.set(optimistic);

	try {
		const result = await api.moveQueueTrack(itemId, target);
		playbackQueue.set(result.queue);
		playerError.set(null);
	} catch (error) {
		// Roll back on failure.
		playbackQueue.set(before);
		setError('reorder queue', error);
	}
}

export async function clearQueue(): Promise<QueueItem[]> {
	const before = get(playbackQueue);
	try {
		const result = await api.clearQueue();
		playbackQueue.set(result.queue);
		if (result.playback_state) applyState(result.playback_state);
		playerError.set(null);
		// Offer undo via toast.
		const restorable = before.filter(
			(item) => !result.queue.some((q) => q.id === item.id)
		);
		if (restorable.length > 0) {
			showToast(`Queue cleared — Undo (${restorable.length})`, 'info', 6000);
			// We register the undo handler externally — caller (layout) wires the toast click.
		}
		return restorable;
	} catch (error) {
		setError('clear queue', error);
		return [];
	}
}

export async function restoreQueueItems(items: QueueItem[]): Promise<void> {
	if (!items.length) return;
	try {
		// Re-add each track in order. addQueueTrack appends to the end.
		for (const item of items) {
			if (item.track.id > 0) {
				await api.addQueueTrack(item.track.id);
			}
		}
		const snapshot = await api.getPlaybackState();
		playbackQueue.set(snapshot.queue);
		playerError.set(null);
	} catch (error) {
		setError('restore queue', error);
	}
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
	// Ephemeral Tidal tracks have a negative id (= -tidal_id) and no local DB row yet.
	// Import them first so we have a real id to favorite.
	if (trackId < 0) {
		const ephemeral = get(currentTrack);
		if (!ephemeral || !ephemeral.tidal_id) return;
		try {
			const { local_id } = await api.importTidalTrackForRadio({
				tidal_id: ephemeral.tidal_id,
				title: ephemeral.title ?? 'Unknown',
				artist_name: ephemeral.artist_name,
				album_title: ephemeral.album_title,
				artwork_url: ephemeral.artwork_url,
				duration_ms: ephemeral.duration_ms,
			});
			currentTrack.update((t) => (t && t.id === trackId ? { ...t, id: local_id } : t));
			playbackQueue.update((q) =>
				q.map((item) =>
					item.track.id === trackId ? { ...item, track: { ...item.track, id: local_id } } : item
				)
			);
			return toggleTrackFavorite(local_id, false);
		} catch (error) {
			setError('like that track', error);
			throw error;
		}
	}

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
	},
) {
	if (trackIds.length === 0) return;
	if (!assertOnline()) return;
	playerError.set(null);
	if (!options?.preserveRadioReasons) clearRadioReasons();
	try {
		await api.replacePlaybackQueue(trackIds, options?.reasons, options?.pendingCandidates);
		const snapshot = await api.playTrack(trackIds[0]);
		hydratePlayback(snapshot);
	} catch (error) {
		setError('start playback', error, () => loadQueueAndPlay(trackIds));
	}
}

function shuffleArray<T>(items: T[]): T[] {
	const arr = items.slice();
	for (let i = arr.length - 1; i > 0; i--) {
		const j = Math.floor(Math.random() * (i + 1));
		[arr[i], arr[j]] = [arr[j], arr[i]];
	}
	return arr;
}

export async function playAlbum(albumId: number, startTrackId?: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	try {
		const { tracks } = await api.getAlbumTracks(albumId);
		if (tracks.length === 0) {
			playerError.set({ message: 'Album has no tracks.' });
			return;
		}
		const ordered = startTrackId
			? [
					...tracks.filter((t) => t.id === startTrackId),
					...tracks.filter((t) => t.id !== startTrackId)
				]
			: tracks;
		await loadQueueAndPlay(ordered.map((t) => t.id));
	} catch (error) {
		setError('play that album', error, () => playAlbum(albumId, startTrackId));
	}
}

export async function shuffleAlbum(albumId: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	try {
		const { tracks } = await api.getAlbumTracks(albumId);
		if (tracks.length === 0) {
			playerError.set({ message: 'Album has no tracks.' });
			return;
		}
		const shuffled = shuffleArray(tracks);
		await loadQueueAndPlay(shuffled.map((t) => t.id));
	} catch (error) {
		setError('shuffle that album', error, () => shuffleAlbum(albumId));
	}
}

export async function playArtist(artistId: number, startTrackId?: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	try {
		const { tracks } = await api.getArtistTracks(artistId);
		if (tracks.length === 0) {
			playerError.set({ message: 'Artist has no tracks.' });
			return;
		}
		const ordered = startTrackId
			? [
					...tracks.filter((t) => t.id === startTrackId),
					...tracks.filter((t) => t.id !== startTrackId)
				]
			: tracks;
		await loadQueueAndPlay(ordered.map((t) => t.id));
	} catch (error) {
		setError('play that artist', error, () => playArtist(artistId, startTrackId));
	}
}

export async function shuffleArtist(artistId: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	try {
		const { tracks } = await api.getArtistTracks(artistId);
		if (tracks.length === 0) {
			playerError.set({ message: 'Artist has no tracks.' });
			return;
		}
		await loadQueueAndPlay(shuffleArray(tracks).map((t) => t.id));
	} catch (error) {
		setError('shuffle that artist', error, () => shuffleArtist(artistId));
	}
}

async function startSongRadioFromLibraryTrack(seedTrackId: number) {
	const result = await api.startRadioStart({ seed_track_id: seedTrackId, limit: 60 });
	hydratePlayback({ state: result.state, queue: result.queue });
}

export async function startSongRadio(seedTrackId: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	// Server-side fallback (artist.getsimilar when track-level recall is empty)
	// can take a few seconds. Surface a loading toast so the user knows the
	// click registered. Dismissed before the success/error toast lands.
	const loadingToastId = showToast('Starting Song Radio...', 'info', 8000);
	try {
		await startSongRadioFromLibraryTrack(seedTrackId);

		dismissToast(loadingToastId);
		showToast('Song Radio started', 'success');
	} catch (error) {
		dismissToast(loadingToastId);
		setError('start radio', error, () => startSongRadio(seedTrackId));
	}
}

export async function shufflePlaylist(
	tracks: { id: number }[],
) {
	if (!tracks.length) return;
	const shuffled = shuffleArray([...tracks]);
	await loadQueueAndPlay(shuffled.map((t) => t.id));
	showToast('Shuffling playlist', 'success');
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
	try {
		const { tracks } = await api.getTidalPlaylistTracks(tidalUuid);
		if (!tracks.length) {
			playerError.set({ message: 'No playable tracks in this playlist.' });
			return;
		}
		// TODO: queue remaining tracks once TIDAL ephemeral queue is supported (see addTidalTrackToQueue stub)
		await playTidalTrackNow(tracks[0]);
		showToast('Playing TIDAL playlist', 'success');
	} catch (error) {
		setError('load TIDAL playlist', error, () => playTidalPlaylist(tidalUuid));
	}
}

export async function startArtistRadio(artistId: number, _seedTrackId?: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	try {
		const queue = await api.startRadioArtist({ seed_artist_id: artistId, limit: 60 });
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
		setError('start artist radio', error, () => startArtistRadio(artistId, _seedTrackId));
	}
}

export async function startAlbumRadio(albumId: number) {
	if (!assertOnline()) return;
	playerError.set(null);
	try {
		const queue = await api.startRadioAlbum({ seed_album_id: albumId, limit: 60 });
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
		setError('start album radio', error, () => startAlbumRadio(albumId));
	}
}

export async function playTrackNext(trackId: number) {
	playerError.set(null);
	// Add to queue, then move next to the currently-playing track.
	try {
		const addResult = await api.addQueueTrack(trackId);
		playbackQueue.set(addResult.queue);
		const justAdded = addResult.queue.find((item) => item.track.id === trackId);
		if (justAdded) {
			await moveQueueTrackNext(justAdded.id);
		}
		noteSuccess();
	} catch (error) {
		setError('queue that track', error, () => playTrackNext(trackId));
	}
}

export async function playTidalTrackNow(track: TidalPlayable): Promise<void> {
	playerError.set(null);
	try {
		await api.playTidalTrack(track);
		currentTrack.set({
			id: -track.tidal_id,
			title: track.title,
			artist_id: -1, // no library artist for ephemeral tracks
			artist_name: track.artist_name,
			artist_tidal_id: track.artist_tidal_id ?? null,
			album_id: null,
			album_title: track.album_title,
			disc_number: null,
			track_number: null,
			duration_ms: track.duration_ms,
			isrc: null,
			tidal_id: track.tidal_id,
			best_quality: 'LOSSLESS', // placeholder — ephemeral tracks don't report fidelity
			best_source: 'tidal',
			fidelity_score: 0,
			is_favorite: false,
			play_count: 0,
			last_played_at: null,
			date_added: null,
			source: 'tidal_ephemeral',
			artwork_url: track.artwork_url,
		});
		isPlaying.set(true);
		noteSuccess();
		showToast(`Playing ${trackLabel(track)}`, 'success');
	} catch (error) {
		setError('play that Tidal track', error, () => playTidalTrackNow(track));
		showToast(`Playback failed`, 'error');
	}
}

function tidalQueueRequest(track: TidalPlayable) {
	return {
		kind: 'tidal' as const,
		tidal_id: track.tidal_id,
		artist: track.artist_name ?? 'Unknown Artist',
		title: track.title,
		album_title: track.album_title,
		duration_ms: track.duration_ms
	};
}

function setOptimisticTidalTrack(track: TidalPlayable) {
	currentTrack.set({
		id: -track.tidal_id,
		title: track.title,
		artist_id: -1,
		artist_name: track.artist_name,
		artist_tidal_id: track.artist_tidal_id ?? null,
		album_id: null,
		album_title: track.album_title,
		disc_number: null,
		track_number: null,
		duration_ms: track.duration_ms,
		isrc: null,
		tidal_id: track.tidal_id,
		best_quality: 'LOSSLESS',
		best_source: 'tidal',
		fidelity_score: 0,
		is_favorite: false,
		play_count: 0,
		last_played_at: null,
		date_added: null,
		source: 'tidal_ephemeral',
		artwork_url: track.artwork_url,
	});
	isPlaying.set(true);
}

export async function playTidalTrackNext(track: TidalPlayable): Promise<void> {
	playerError.set(null);
	try {
		const result = await api.queuePlayNext(tidalQueueRequest(track));
		playbackQueue.set(result.queue);
		noteSuccess();
		showToast(`Queued next: ${trackLabel(track)}`, 'success');
	} catch (error) {
		setError('queue that Tidal track next', error, () => playTidalTrackNext(track));
	}
}

export async function playTidalTracksNow(tracks: TidalPlayable[], label = 'playlist'): Promise<void> {
	if (!assertOnline()) return;
	const playable = tracks.filter((track) => track.tidal_id > 0);
	if (!playable.length) {
		showToast('No playable tracks ready yet', 'info');
		return;
	}
	playerError.set(null);
	try {
		setOptimisticTidalTrack(playable[0]);
		await api.playTidalMix(playable);
		noteSuccess();
		showToast(`Playing ${label} (${playable.length} tracks)`, 'success');
	} catch (error) {
		setError('play those Tidal tracks', error, () => playTidalTracksNow(tracks, label));
		showToast('Playback failed', 'error');
	}
}

export async function shuffleTidalTracksNow(tracks: TidalPlayable[], label = 'playlist'): Promise<void> {
	const shuffled = shuffleArray(tracks);
	await playTidalTracksNow(shuffled, label);
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
		playbackQueue.set(result.queue);
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
		playbackQueue.set(result.queue);
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
		playbackQueue.set(result.queue);
		noteSuccess();
		showToast(`Added to queue: ${playable.length} tracks`, 'success');
	} catch (error) {
		setError('add those Tidal tracks to queue', error, () => addTidalTracksToQueue(tracks));
	}
}

export async function playTidalAlbum(tidalAlbumId: number): Promise<void> {
	if (!assertOnline()) return;
	playerError.set(null);
	try {
		const { tracks } = await api.getTidalAlbumTracks(tidalAlbumId);
		if (tracks.length === 0) {
			showToast('Album has no tracks', 'error');
			return;
		}
		const first = tracks[0];
		await playTidalTrackNow({
			tidal_id: first.tidal_id,
			title: first.title,
			artist_name: first.artist_name ?? null,
			album_title: first.album_title ?? null,
			artwork_url: first.artwork_url,
			duration_ms: first.duration_ms,
		});
	} catch (error) {
		setError('play that Tidal album', error, () => playTidalAlbum(tidalAlbumId));
		showToast(`Album playback failed`, 'error');
	}
}

/// Play a TIDAL mix by id — fetches the mix's track list and sends the WHOLE
/// list to the server's `/api/tidal/play-mix` endpoint. The server starts the
/// first track immediately and queues the rest behind it so playback
/// auto-advances through the mix without a per-track round trip.
export async function playTidalMix(mixId: string): Promise<void> {
	if (!assertOnline()) return;
	playerError.set(null);
	try {
		const { tracks } = await api.getTidalMixTracks(mixId);
		if (tracks.length === 0) {
			showToast('Mix has no tracks', 'error');
			return;
		}
		// Optimistically reflect the first track in the now-playing UI so
		// there's no flicker before the server's PlaybackStateChanged event
		// arrives. Server will overwrite with authoritative data on Started.
		const first = tracks[0];
		currentTrack.set({
			id: -first.tidal_id,
			title: first.title,
			artist_id: -1,
			artist_name: first.artist_name ?? null,
			artist_tidal_id: null,
			album_id: null,
			album_title: first.album_title ?? null,
			disc_number: null,
			track_number: null,
			duration_ms: first.duration_ms,
			isrc: null,
			tidal_id: first.tidal_id,
			best_quality: 'LOSSLESS',
			best_source: 'tidal',
			fidelity_score: 0,
			is_favorite: false,
			play_count: 0,
			last_played_at: null,
			date_added: null,
			source: 'tidal_ephemeral',
			artwork_url: first.artwork_url,
		});
		isPlaying.set(true);

		await api.playTidalMix(
			tracks.map((t) => ({
				tidal_id: t.tidal_id,
				title: t.title,
				artist_name: t.artist_name ?? null,
				album_title: t.album_title ?? null,
				artwork_url: t.artwork_url,
				duration_ms: t.duration_ms,
			})),
		);
		noteSuccess();
		showToast(`Playing mix (${tracks.length} tracks queued)`, 'success');
	} catch (error) {
		setError('play that Tidal mix', error, () => playTidalMix(mixId));
		showToast(`Mix playback failed`, 'error');
	}
}

export async function startTidalSongRadio(track: TidalPlayable): Promise<void> {
	if (!assertOnline()) return;
	playerError.set(null);
	const loadingToastId = showToast('Starting Song Radio...', 'info', 8000);
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
			setError('start Tidal radio', error, () => startTidalSongRadio(track));
			return;
		}
	}
	// Try discovery radio seeded directly by Tidal ID (only works if track is already in library)
	try {
		const { tracks } = await api.getRadioTracks({ seed_tidal_id: track.tidal_id, limit: 40 });
		const radioIds = tracks.map((t) => t.track_id);
		if (radioIds.length > 0) {
			await loadQueueAndPlay(radioIds);
			dismissToast(loadingToastId);
			showToast(`Radio from ${trackLabel(track)}`, 'success');
			playerError.set(null);
			return;
		}
	} catch (error) {
		// 404 = track not yet in library index — fall through to silent import.
		// Any other error is a real failure.
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
		await startSongRadioFromLibraryTrack(local_id);
		dismissToast(loadingToastId);
		showToast(`Radio from ${trackLabel(track)}`, 'success');
		playerError.set(null);
	} catch (error) {
		dismissToast(loadingToastId);
		setError('start Tidal radio', error, () => startTidalSongRadio(track));
		showToast(`No radio results for "${trackLabel(track)}"`, 'info');
	}
}
