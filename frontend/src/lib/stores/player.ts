import { get, writable } from 'svelte/store';
import { api, type PlaybackSnapshot, type PlaybackState, type QueueItem, type Track } from '$lib/api/client';

export const currentTrack = writable<Track | null>(null);
export const isPlaying = writable(false);
export const position = writable(0);
export const volume = writable(1.0);
export const automixEnabled = writable(false);
export const automixDiscoverNew = writable(false);

// ─── Client-side position ticker ──────────────────────────────────────────────
// Increments position every second while playing so the progress bar moves
// without polling the server. Position is re-synced from the server on every
// WebSocket playback event.
let _positionTicker: ReturnType<typeof setInterval> | null = null;

function startPositionTicker() {
	if (_positionTicker !== null) return;
	_positionTicker = setInterval(() => {
		position.update((p) => {
			const track = get(currentTrack);
			if (!track?.duration_ms) return p;
			return Math.min(p + 1000, track.duration_ms);
		});
	}, 1000);
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
export const playerError = writable<string | null>(null);

// Cycle: off → genre (Galaxy default) → weighted → true → back to off
const SHUFFLE_SEQUENCE: PlaybackState['shuffle_mode'][] = ['off', 'genre', 'weighted', 'true'];

function applyState(state: PlaybackState) {
	currentTrack.set(state.current_track);
	isPlaying.set(state.is_playing);
	position.set(state.position_ms);
	volume.set(state.volume);
	shuffleMode.set(state.shuffle_mode);
	repeatMode.set(state.repeat_mode);
	automixEnabled.set(state.automix_enabled);
	crossfadeMs.set(state.crossfade_ms);
	automixDiscoverNew.set(state.automix_discover_new);
}

export function hydratePlayback(snapshot: PlaybackSnapshot) {
	applyState(snapshot.state);
	playbackQueue.set(snapshot.queue);
	playerReady.set(true);
	playerError.set(null);
}

export async function refreshPlaybackState() {
	try {
		const snapshot = await api.getPlaybackState();
		hydratePlayback(snapshot);
	} catch (error) {
		playerError.set(`Failed to load playback state: ${error}`);
	}
}

export async function playTrackNow(trackId: number) {
	try {
		const snapshot = await api.playTrack(trackId);
		hydratePlayback(snapshot);
		playerError.set(null);
	} catch (error) {
		playerError.set(`Playback failed: ${error}`);
	}
}

export async function togglePlayback() {
	try {
		if (get(isPlaying)) {
			const result = await api.pausePlayback();
			applyState(result.state);
		} else {
			const result = await api.resumePlayback();
			applyState(result.state);
		}
		playerError.set(null);
	} catch (error) {
		playerError.set(`Playback control failed: ${error}`);
	}
}

export async function playPreviousTrack() {
	try {
		const snapshot = await api.previousTrack();
		hydratePlayback(snapshot);
		playerError.set(null);
	} catch (error) {
		playerError.set(`Failed to go to previous track: ${error}`);
	}
}

export async function playNextTrack() {
	try {
		const snapshot = await api.nextTrack();
		hydratePlayback(snapshot);
		playerError.set(null);
	} catch (error) {
		playerError.set(`Failed to advance to next track: ${error}`);
	}
}

export async function setPlayerVolume(nextVolume: number) {
	try {
		const result = await api.setPlaybackVolume(nextVolume);
		// Only sync volume — applying full state would overwrite the local position
		// ticker with a slightly stale server value, causing the displayed time to jump.
		volume.set(result.state.volume);
	} catch (error) {
		playerError.set(`Failed to set volume: ${error}`);
	}
}

export async function setPlayerPosition(nextPositionMs: number) {
	try {
		const result = await api.setPlaybackPosition(nextPositionMs);
		applyState(result.state);
	} catch (error) {
		playerError.set(`Failed to seek: ${error}`);
	}
}

export async function setPlayerRepeatMode(mode: PlaybackState['repeat_mode']) {
	try {
		const result = await api.setPlaybackRepeat(mode);
		applyState(result.state);
		playerError.set(null);
	} catch (error) {
		playerError.set(`Failed to set repeat mode: ${error}`);
	}
}

export async function cyclePlayerRepeatMode() {
	const sequence: PlaybackState['repeat_mode'][] = ['off', 'all', 'one'];
	const current = get(repeatMode);
	const next = sequence[(sequence.indexOf(current) + 1) % sequence.length];
	await setPlayerRepeatMode(next);
}

export async function setPlayerShuffleMode(mode: PlaybackState['shuffle_mode']) {
	try {
		const snapshot = await api.setPlaybackShuffle(mode);
		hydratePlayback(snapshot);
		playerError.set(null);
	} catch (error) {
		playerError.set(`Failed to set shuffle mode: ${error}`);
	}
}

export async function setPlayerAutomixEnabled(enabled: boolean, crossfade_ms?: number, discover_new?: boolean) {
	try {
		const result = await api.setPlaybackAutomix(enabled, crossfade_ms, discover_new);
		// Only sync automix fields — applying full state would clobber the local position ticker.
		automixEnabled.set(result.state.automix_enabled);
		crossfadeMs.set(result.state.crossfade_ms);
		automixDiscoverNew.set(result.state.automix_discover_new);
		if (result.queue) playbackQueue.set(result.queue);
		playerError.set(null);
	} catch (error) {
		playerError.set(`Failed to set automix: ${error}`);
	}
}

export async function setPlayerCrossfadeMs(ms: number) {
	await setPlayerAutomixEnabled(get(automixEnabled), ms);
}

export async function setPlayerDiscoverNew(enabled: boolean) {
	await setPlayerAutomixEnabled(get(automixEnabled), get(crossfadeMs), enabled);
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

export async function addTrackToQueue(trackId: number) {
	const result = await api.addQueueTrack(trackId);
	playbackQueue.set(result.queue);
	playerError.set(null);
}

export async function moveQueueTrackNext(queueItemId: number) {
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

	const result = await api.replacePlaybackQueue(queue.map((item) => item.track.id));
	playbackQueue.set(result.queue);
	playerError.set(null);
}

export async function removeTrackFromQueue(queueItemId: number) {
	const result = await api.removeQueueTrack(queueItemId);
	playbackQueue.set(result.queue);
	playerError.set(null);
}

export function setTrackFavoriteStatus(trackId: number, favorite: boolean) {
	currentTrack.update((track) =>
		track && track.id === trackId ? { ...track, is_favorite: favorite } : track
	);
	playbackQueue.update((queue) =>
		queue.map((item) =>
			item.track.id === trackId
				? {
						...item,
						track: { ...item.track, is_favorite: favorite }
					}
				: item
		)
	);
}

export async function toggleTrackFavorite(trackId: number) {
	const current = get(currentTrack);
	const queued = get(playbackQueue).find((item) => item.track.id === trackId)?.track ?? null;
	const activeTrack = current?.id === trackId ? current : queued;
	if (!activeTrack) return;

	const nextFavorite = !activeTrack.is_favorite;

	try {
		await api.setTrackFavorite(trackId, nextFavorite);
		setTrackFavoriteStatus(trackId, nextFavorite);
		playerError.set(null);
	} catch (error) {
		playerError.set(`Failed to ${nextFavorite ? 'like' : 'unlike'} track: ${error}`);
		throw error;
	}
}
