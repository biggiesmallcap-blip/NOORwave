import { get } from 'svelte/store';
import { describe, expect, it, vi, beforeEach } from 'vitest';
import type { PlaybackSnapshot, PlaybackState, QueueItem, TidalPlayable, Track } from '$lib/api/client';

// Mock the api client so we can drive restoreQueueItems without a server.
// vi.mock is hoisted, so the factory has to declare its own spies, then we
// re-import them from the module to assert against.
vi.mock('$lib/api/client', async () => {
	const addQueueTrack = vi.fn(async () => ({ queue: [], playback_state: null }));
	const queueAppend = vi.fn(async () => ({ queue: [], playback_state: null }));
	const getPlaybackState = vi.fn(async () => ({ queue: [] }));
	const removeQueueTrack = vi.fn(async () => ({ queue: [], playback_state: null }));
	const getTrackAudioFeatures = vi.fn(async () => ({ features: null }));
	const playTrack = vi.fn();
	const playTidalTrack = vi.fn();
	const getAlbumTracks = vi.fn();
	const replacePlaybackQueue = vi.fn();
	const startRadioStart = vi.fn();
	return {
		api: {
			addQueueTrack,
			queueAppend,
			getPlaybackState,
			removeQueueTrack,
			getTrackAudioFeatures,
			playTrack,
			playTidalTrack,
			getAlbumTracks,
			replacePlaybackQueue,
			startRadioStart,
		},
		ApiError: class ApiError extends Error {},
	};
});

import { api } from '$lib/api/client';
import {
	currentQueueItemId,
	currentTrack,
	isPlaying,
	lastSuccessfulCallAt,
	playAlbum,
	startSongRadio,
	playTrackNow,
	playTidalTrackNow,
	playbackQueue,
	playerError,
	refreshPlaybackState,
	removeTrackFromQueue,
	restoreQueueItems,
} from './player';

type AlbumTracksResult = Awaited<ReturnType<typeof api.getAlbumTracks>>;
type StartRadioResult = Awaited<ReturnType<typeof api.startRadioStart>>;

function libraryRow(id: number, trackId: number): QueueItem {
	return {
		id,
		position: id,
		source: 'library',
		track: {
			id: trackId,
			title: `Library ${trackId}`,
			tidal_id: null,
			source: 'tidal_stream',
		} as QueueItem['track'],
	};
}

function ephemeralTidalRow(queueId: number, tidalId: number): QueueItem {
	return {
		id: queueId,
		position: queueId,
		source: 'tidal_mix',
		track: {
			id: -tidalId,
			title: `Tidal ${tidalId}`,
			tidal_id: tidalId,
			source: 'tidal_stream',
			artist_name: null,
			album_title: null,
			artwork_url: null,
			duration_ms: 200_000,
		} as QueueItem['track'],
	};
}

function pendingRow(queueId: number): QueueItem {
	return {
		id: queueId,
		position: queueId,
		source: 'radio_pending',
		is_pending: true,
		track: {
			id: 0,
			title: 'Pending Title',
			tidal_id: null,
		} as QueueItem['track'],
	};
}

function playbackState(current: QueueItem): PlaybackState {
	return {
		current_track: current.track,
		current_queue_item_id: current.id,
		position_ms: 0,
		is_playing: true,
		volume: 0.8,
		shuffle_mode: 'off',
		repeat_mode: 'off',
		automix_enabled: false,
		crossfade_ms: 0,
		automix_discover_new: false,
		automix_use_learning: false,
		automix_allow_external: false,
	};
}

function playbackSnapshot(current: QueueItem): PlaybackSnapshot {
	return {
		state: playbackState(current),
		queue: [current],
	};
}

function radioQueue(current: QueueItem): StartRadioResult {
	return {
		state: playbackState(current),
		queue: [current],
		first_playable: {
			type: 'library',
			queue_item_id: current.id,
			track_id: current.track.id,
		},
	};
}

function libraryTrack(trackId: number): Track {
	return libraryRow(trackId, trackId).track as Track;
}

function tidalPlayable(tidalId: number): TidalPlayable {
	return {
		tidal_id: tidalId,
		title: `Tidal ${tidalId}`,
		artist_name: `Artist ${tidalId}`,
		album_title: null,
		artwork_url: null,
		duration_ms: 200_000,
	};
}

function deferred<T>() {
	let resolve!: (value: T) => void;
	let reject!: (reason?: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

describe('restoreQueueItems', () => {
	beforeEach(() => {
		vi.mocked(api.addQueueTrack).mockClear();
		vi.mocked(api.queueAppend).mockClear();
		vi.mocked(api.getPlaybackState).mockClear();
		vi.mocked(api.removeQueueTrack).mockClear();
		vi.mocked(api.getTrackAudioFeatures).mockClear();
		vi.mocked(api.playTrack).mockClear();
		vi.mocked(api.playTidalTrack).mockClear();
		vi.mocked(api.getAlbumTracks).mockClear();
		vi.mocked(api.replacePlaybackQueue).mockClear();
		vi.mocked(api.startRadioStart).mockClear();
		currentTrack.set(null);
		currentQueueItemId.set(null);
		isPlaying.set(false);
		playbackQueue.set([]);
		playerError.set(null);
	});

	it('restores a library-only queue via addQueueTrack', async () => {
		const summary = await restoreQueueItems([libraryRow(1, 100), libraryRow(2, 101)]);
		expect(summary.restored).toBe(2);
		expect(summary.skipped).toBe(0);
		expect(api.addQueueTrack).toHaveBeenCalledTimes(2);
		expect(api.queueAppend).not.toHaveBeenCalled();
	});

	it('restores an ephemeral TIDAL row via queueAppend, preserving the tidal_id', async () => {
		const summary = await restoreQueueItems([ephemeralTidalRow(50, 999)]);
		expect(summary.restored).toBe(1);
		expect(api.queueAppend).toHaveBeenCalledTimes(1);
		const callArg = vi.mocked(api.queueAppend).mock.calls[0][0] as {
			tidal_id: number;
		};
		expect(callArg.tidal_id).toBe(999);
		expect(api.addQueueTrack).not.toHaveBeenCalled();
	});

	it('preserves original order across a mixed library + TIDAL queue', async () => {
		const mixed = [
			libraryRow(1, 100),
			ephemeralTidalRow(2, 200),
			libraryRow(3, 101),
			ephemeralTidalRow(4, 201),
		];
		const summary = await restoreQueueItems(mixed);
		expect(summary.restored).toBe(4);
		expect(summary.skipped).toBe(0);

		// Each row is restored exactly once.
		expect(api.addQueueTrack).toHaveBeenCalledTimes(2);
		expect(api.queueAppend).toHaveBeenCalledTimes(2);

		// Verify the issue order matches the input order. addQueueTrack and
		// queueAppend are interleaved; we check the relative ordering of all
		// four mock calls via their invocation order on the spy timeline.
		const orderTrace: string[] = [];
		vi.mocked(api.addQueueTrack).mock.invocationCallOrder.forEach((n) =>
			orderTrace.push(`add@${n}`)
		);
		vi.mocked(api.queueAppend).mock.invocationCallOrder.forEach((n) =>
			orderTrace.push(`tidal@${n}`)
		);
		orderTrace.sort((a, b) => Number(a.split('@')[1]) - Number(b.split('@')[1]));
		expect(orderTrace.map((t) => t.split('@')[0])).toEqual([
			'add',
			'tidal',
			'add',
			'tidal',
		]);
	});

	it('skips pending rows but still restores resolved rows in the same batch', async () => {
		const summary = await restoreQueueItems([
			libraryRow(1, 100),
			pendingRow(2),
			ephemeralTidalRow(3, 300),
		]);
		expect(summary.restored).toBe(2);
		expect(summary.skipped).toBe(1);
		expect(api.addQueueTrack).toHaveBeenCalledTimes(1);
		expect(api.queueAppend).toHaveBeenCalledTimes(1);
	});

	it('no-ops on empty input without hitting the API', async () => {
		const summary = await restoreQueueItems([]);
		expect(summary).toEqual({ restored: 0, skipped: 0 });
		expect(api.addQueueTrack).not.toHaveBeenCalled();
		expect(api.queueAppend).not.toHaveBeenCalled();
	});
});

describe('removeTrackFromQueue', () => {
	beforeEach(() => {
		vi.mocked(api.removeQueueTrack).mockClear();
		vi.mocked(api.getTrackAudioFeatures).mockClear();
		currentTrack.set(null);
		currentQueueItemId.set(null);
		isPlaying.set(false);
		playbackQueue.set([]);
	});

	it('applies playback state returned by the remove endpoint', async () => {
		const next = libraryRow(20, 2);
		vi.mocked(api.removeQueueTrack).mockResolvedValueOnce({
			queue: [next],
			playback_state: playbackState(next),
		});

		await removeTrackFromQueue(10);

		expect(api.removeQueueTrack).toHaveBeenCalledWith(10);
		expect(get(playbackQueue)).toEqual([next]);
		expect(get(currentTrack)?.id).toBe(2);
		expect(get(currentQueueItemId)).toBe(20);
		expect(get(isPlaying)).toBe(true);
		expect(api.getTrackAudioFeatures).toHaveBeenCalledWith(2);
	});
});

describe('stale playback responses', () => {
	beforeEach(() => {
		vi.mocked(api.getPlaybackState).mockClear();
		vi.mocked(api.getTrackAudioFeatures).mockClear();
		vi.mocked(api.playTrack).mockClear();
		vi.mocked(api.playTidalTrack).mockClear();
		vi.mocked(api.getAlbumTracks).mockClear();
		vi.mocked(api.replacePlaybackQueue).mockClear();
		vi.mocked(api.startRadioStart).mockClear();
		currentTrack.set(null);
		currentQueueItemId.set(null);
		isPlaying.set(false);
		playbackQueue.set([]);
		playerError.set(null);
		lastSuccessfulCallAt.set(Date.now());
	});

	it('ignores an older play response after a newer play request wins', async () => {
		const older = deferred<PlaybackSnapshot>();
		const newer = deferred<PlaybackSnapshot>();
		const olderRow = libraryRow(10, 1);
		const newerRow = libraryRow(20, 2);
		vi.mocked(api.playTrack)
			.mockReturnValueOnce(older.promise)
			.mockReturnValueOnce(newer.promise);

		const olderAction = playTrackNow(1);
		const newerAction = playTrackNow(2);

		newer.resolve(playbackSnapshot(newerRow));
		await newerAction;
		expect(get(currentTrack)?.id).toBe(2);

		older.resolve(playbackSnapshot(olderRow));
		await olderAction;
		expect(get(currentTrack)?.id).toBe(2);
		expect(get(playerError)).toBeNull();
	});

	it('ignores an older passive refresh after a newer play request', async () => {
		const refresh = deferred<PlaybackSnapshot>();
		const play = deferred<PlaybackSnapshot>();
		const staleRow = libraryRow(10, 1);
		const currentRow = libraryRow(20, 2);
		vi.mocked(api.getPlaybackState).mockReturnValueOnce(refresh.promise);
		vi.mocked(api.playTrack).mockReturnValueOnce(play.promise);

		const refreshAction = refreshPlaybackState();
		const playAction = playTrackNow(2);

		play.resolve(playbackSnapshot(currentRow));
		await playAction;
		expect(get(currentTrack)?.id).toBe(2);

		refresh.resolve(playbackSnapshot(staleRow));
		await refreshAction;
		expect(get(currentTrack)?.id).toBe(2);
		expect(get(playerError)).toBeNull();
	});

	it('does not let an older album track fetch start a stale queue replace', async () => {
		const older = deferred<AlbumTracksResult>();
		const newer = deferred<AlbumTracksResult>();
		vi.mocked(api.getAlbumTracks)
			.mockReturnValueOnce(older.promise)
			.mockReturnValueOnce(newer.promise);
		vi.mocked(api.replacePlaybackQueue).mockImplementation(async (trackIds: number[]) => ({
			queue: trackIds.map((trackId, index) => libraryRow(index + 1, trackId)),
			shuffle_debug: null,
		}));
		vi.mocked(api.playTrack).mockImplementation(async (trackId: number) =>
			playbackSnapshot(libraryRow(trackId, trackId))
		);

		const olderAction = playAlbum(1);
		const newerAction = playAlbum(2);

		newer.resolve({ tracks: [libraryTrack(2)], tidal_tracks: [], album_tidal_id: null });
		await newerAction;
		expect(get(currentTrack)?.id).toBe(2);
		expect(api.replacePlaybackQueue).toHaveBeenCalledTimes(1);
		expect(vi.mocked(api.replacePlaybackQueue).mock.calls[0][0]).toEqual([2]);

		older.resolve({ tracks: [libraryTrack(1)], tidal_tracks: [], album_tidal_id: null });
		await olderAction;
		expect(get(currentTrack)?.id).toBe(2);
		expect(api.replacePlaybackQueue).toHaveBeenCalledTimes(1);
	});

	it('ignores an older direct TIDAL play response after a newer TIDAL play', async () => {
		const older = deferred<void>();
		const newer = deferred<void>();
		vi.mocked(api.playTidalTrack)
			.mockReturnValueOnce(older.promise)
			.mockReturnValueOnce(newer.promise);

		const olderAction = playTidalTrackNow(tidalPlayable(1));
		const newerAction = playTidalTrackNow(tidalPlayable(2));

		newer.resolve();
		await newerAction;
		expect(get(currentTrack)?.tidal_id).toBe(2);
		expect(get(currentTrack)?.id).toBe(-2);

		older.resolve();
		await olderAction;
		expect(get(currentTrack)?.tidal_id).toBe(2);
		expect(get(currentTrack)?.id).toBe(-2);
		expect(get(playerError)).toBeNull();
	});

	it('ignores an older song radio response after a newer play request', async () => {
		const radio = deferred<StartRadioResult>();
		const play = deferred<PlaybackSnapshot>();
		const staleRow = libraryRow(10, 1);
		const currentRow = libraryRow(20, 2);
		vi.mocked(api.startRadioStart).mockReturnValueOnce(radio.promise);
		vi.mocked(api.playTrack).mockReturnValueOnce(play.promise);

		const radioAction = startSongRadio(1);
		const playAction = playTrackNow(2);

		play.resolve(playbackSnapshot(currentRow));
		await playAction;
		expect(get(currentTrack)?.id).toBe(2);

		radio.resolve(radioQueue(staleRow));
		await radioAction;
		expect(get(currentTrack)?.id).toBe(2);
		expect(get(playerError)).toBeNull();
	});
});
