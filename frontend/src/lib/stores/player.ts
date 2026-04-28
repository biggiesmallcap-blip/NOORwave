import { get, writable } from 'svelte/store';
import {
	api,
	type AudioDspFeatures,
	type PlaybackSnapshot,
	type PlaybackState,
	type QueueItem,
	type StreamDisplayInfo,
	type TidalPlayable,
	type Track
} from '$lib/api/client';
import { showToast } from '$lib/stores/toast';

function trackLabel(track: { title?: string | null; artist_name?: string | null }): string {
	const t = (track.title ?? '').trim();
	const a = (track.artist_name ?? '').trim();
	if (t && a) return `${t} — ${a}`;
	return t || a || 'track';
}

export const currentTrack = writable<Track | null>(null);
export const currentTrackFeatures = writable<AudioDspFeatures | null>(null);
export const currentStreamDisplay = writable<StreamDisplayInfo | null>(null);

export async function refreshPlaybackRuntime() {
	try {
		const result = await api.getPlaybackRuntime();
		currentStreamDisplay.set(result.stream ?? null);
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
export const playerError = writable<string | null>(null);

// Cycle: off → genre (Galaxy default) → weighted → true → back to off
const SHUFFLE_SEQUENCE: PlaybackState['shuffle_mode'][] = ['off', 'genre', 'weighted', 'true'];

function applyState(state: PlaybackState) {
	currentTrack.set(state.current_track);
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

export async function setPlayerAutomixEnabled(
	enabled: boolean,
	crossfade_ms?: number,
	discover_new?: boolean,
	use_learning?: boolean,
	allow_external?: boolean
) {
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

export async function addTrackToQueue(trackId: number) {
	try {
		const result = await api.addQueueTrack(trackId);
		playbackQueue.set(result.queue);
		playerError.set(null);
		showToast('Added to queue', 'success');
	} catch (error) {
		playerError.set(`Failed to add to queue: ${error}`);
		throw error;
	}
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

export async function toggleTrackFavorite(trackId: number, currentIsFavorite?: boolean) {
	let nextFavorite: boolean;
	if (currentIsFavorite !== undefined) {
		nextFavorite = !currentIsFavorite;
	} else {
		const current = get(currentTrack);
		const queued = get(playbackQueue).find((item) => item.track.id === trackId)?.track ?? null;
		const activeTrack = current?.id === trackId ? current : queued;
		if (!activeTrack) return;
		nextFavorite = !activeTrack.is_favorite;
	}

	// Optimistic flip — UI updates immediately, rollback on rejection.
	setTrackFavoriteStatus(trackId, nextFavorite);
	try {
		await api.setTrackFavorite(trackId, nextFavorite);
		playerError.set(null);
	} catch (error) {
		// Roll back the optimistic update.
		setTrackFavoriteStatus(trackId, !nextFavorite);
		playerError.set(`Failed to ${nextFavorite ? 'like' : 'unlike'} track: ${error}`);
		throw error;
	}
}

// ─── "Start from here" actions ────────────────────────────────────────────────
// Shared helper: replace the queue with the given track IDs and begin playback
// at the first one. Order matters — the first ID in `trackIds` is played first.
async function loadQueueAndPlay(trackIds: number[]) {
	if (trackIds.length === 0) return;
	try {
		await api.replacePlaybackQueue(trackIds);
		const snapshot = await api.playTrack(trackIds[0]);
		hydratePlayback(snapshot);
		playerError.set(null);
	} catch (error) {
		playerError.set(`Failed to start playback: ${error}`);
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
	try {
		const { tracks } = await api.getAlbumTracks(albumId);
		if (tracks.length === 0) {
			playerError.set('Album has no tracks.');
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
		playerError.set(`Failed to play album: ${error}`);
	}
}

export async function shuffleAlbum(albumId: number) {
	try {
		const { tracks } = await api.getAlbumTracks(albumId);
		if (tracks.length === 0) {
			playerError.set('Album has no tracks.');
			return;
		}
		const shuffled = shuffleArray(tracks);
		await loadQueueAndPlay(shuffled.map((t) => t.id));
	} catch (error) {
		playerError.set(`Failed to shuffle album: ${error}`);
	}
}

export async function playArtist(artistId: number, startTrackId?: number) {
	try {
		const { tracks } = await api.getArtistTracks(artistId);
		if (tracks.length === 0) {
			playerError.set('Artist has no tracks.');
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
		playerError.set(`Failed to play artist: ${error}`);
	}
}

export async function shuffleArtist(artistId: number) {
	try {
		const { tracks } = await api.getArtistTracks(artistId);
		if (tracks.length === 0) {
			playerError.set('Artist has no tracks.');
			return;
		}
		await loadQueueAndPlay(shuffleArray(tracks).map((t) => t.id));
	} catch (error) {
		playerError.set(`Failed to shuffle artist: ${error}`);
	}
}

export async function startSongRadio(seedTrackId: number) {
	try {
		const queue = await api.startRadioSong({ seed_track_id: seedTrackId, limit: 60 });
		const radioIds = queue.tracks
			.filter((t) => t.is_in_library && t.track_id > 0)
			.map((t) => t.track_id)
			.filter((id) => id !== seedTrackId);
		await loadQueueAndPlay([seedTrackId, ...radioIds]);
		showToast(`Radio from ${queue.seed.title}`, 'success');
	} catch (error) {
		playerError.set(`Failed to start radio: ${error}`);
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
	try {
		const { tracks } = await api.getTidalPlaylistTracks(tidalUuid);
		if (!tracks.length) {
			playerError.set('No playable tracks in this playlist.');
			return;
		}
		// TODO: queue remaining tracks once TIDAL ephemeral queue is supported (see addTidalTrackToQueue stub)
		await playTidalTrackNow(tracks[0]);
		showToast('Playing TIDAL playlist', 'success');
	} catch (error) {
		playerError.set(`Failed to load TIDAL playlist: ${error}`);
	}
}

export async function startArtistRadio(artistId: number, _seedTrackId?: number) {
	try {
		const queue = await api.startRadioArtist({ seed_artist_id: artistId, limit: 60 });
		const radioIds = queue.tracks
			.filter((t) => t.is_in_library && t.track_id > 0)
			.map((t) => t.track_id);
		if (radioIds.length === 0) {
			playerError.set('No library tracks found for radio.');
			return;
		}
		await loadQueueAndPlay(radioIds);
		showToast(`Radio from ${queue.seed.title}`, 'success');
	} catch (error) {
		playerError.set(`Failed to start artist radio: ${error}`);
	}
}

export async function startAlbumRadio(albumId: number) {
	try {
		const queue = await api.startRadioAlbum({ seed_album_id: albumId, limit: 60 });
		const radioIds = queue.tracks
			.filter((t) => t.is_in_library && t.track_id > 0)
			.map((t) => t.track_id);
		if (radioIds.length === 0) {
			playerError.set('No library tracks found for radio.');
			return;
		}
		await loadQueueAndPlay(radioIds);
		showToast(`Radio from ${queue.seed.title}`, 'success');
	} catch (error) {
		playerError.set(`Failed to start album radio: ${error}`);
	}
}

export async function playTrackNext(trackId: number) {
	// Add to queue, then move next to the currently-playing track.
	try {
		const addResult = await api.addQueueTrack(trackId);
		playbackQueue.set(addResult.queue);
		const justAdded = addResult.queue.find((item) => item.track.id === trackId);
		if (justAdded) {
			await moveQueueTrackNext(justAdded.id);
		}
		playerError.set(null);
	} catch (error) {
		playerError.set(`Failed to queue track: ${error}`);
	}
}

export async function playTidalTrackNow(track: TidalPlayable): Promise<void> {
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
		playerError.set(null);
		showToast(`Playing ${trackLabel(track)}`, 'success');
	} catch (error) {
		playerError.set(`Tidal playback failed: ${error}`);
		showToast(`Playback failed`, 'error');
	}
}

// TODO(v2): true queue insertion for ephemeral tracks requires backend queue support
export async function playTidalTrackNext(track: TidalPlayable): Promise<void> {
	await playTidalTrackNow(track);
}

// TODO(v2): true queue insertion for ephemeral tracks requires backend queue support
export async function addTidalTrackToQueue(track: TidalPlayable): Promise<void> {
	await playTidalTrackNow(track);
}

export async function playTidalAlbum(tidalAlbumId: number): Promise<void> {
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
		playerError.set(`Tidal album playback failed: ${error}`);
		showToast(`Album playback failed`, 'error');
	}
}

export async function startTidalSongRadio(track: TidalPlayable): Promise<void> {
	// Try discovery radio seeded directly by Tidal ID (only works if track is already in library)
	try {
		const { tracks } = await api.getRadioTracks({ seed_tidal_id: track.tidal_id, limit: 40 });
		const radioIds = tracks.map((t) => t.track_id);
		if (radioIds.length > 0) {
			await loadQueueAndPlay(radioIds);
			showToast(`Radio from ${trackLabel(track)}`, 'success');
			playerError.set(null);
			return;
		}
	} catch (error) {
		// 404 = track not yet in library index — fall through to silent import.
		// Any other error is a real failure.
		if (!(error instanceof Error && error.message.includes('404'))) {
			playerError.set(`Tidal radio failed: ${error}`);
			showToast(`Radio failed`, 'error');
			return;
		}
	}

	// Fallback: silently import the track as a tidal_stream entry (invisible in library grids)
	// so the radio engine can use it as a seed, then run song radio from the resulting local ID.
	try {
		const { local_id } = await api.importTidalTrackForRadio(track);
		await startSongRadio(local_id);
		playerError.set(null);
	} catch {
		showToast(`No radio results for "${trackLabel(track)}"`, 'info');
		playerError.set(null);
	}
}
