import { get } from 'svelte/store';
import { describe, expect, it, vi, beforeEach } from 'vitest';
import type {
	PlaybackSnapshot,
	PlaybackState,
	QueueItem,
	TidalPlayable,
	Track
} from '$lib/api/client';

// Mock the api client so we can drive restoreQueueItems without a server.
// vi.mock is hoisted, so the factory has to declare its own spies, then we
// re-import them from the module to assert against.
vi.mock('$lib/api/client', async () => {
	const addQueueTrack = vi.fn(async () => ({ queue: [], playback_state: null }));
	const queueAppend = vi.fn(async () => ({ queue: [], playback_state: null }));
	const getPlaybackState = vi.fn(async () => ({ queue: [] }));
	const removeQueueTrack = vi.fn(async () => ({ queue: [], playback_state: null }));
	const moveQueueTrack = vi.fn(async () => ({ queue: [], playback_state: null }));
	const getTrackAudioFeatures = vi.fn(async () => ({ features: null }));
	const pausePlayback = vi.fn();
	const resumePlayback = vi.fn();
	const playTrack = vi.fn();
	const importTidalTrackForRadio = vi.fn();
	const getAlbumTracks = vi.fn();
	const replacePlaybackQueue = vi.fn();
	const startRadioStart = vi.fn();
	const getRadioTracks = vi.fn();
	return {
		api: {
			addQueueTrack,
			queueAppend,
			getPlaybackState,
			removeQueueTrack,
			moveQueueTrack,
			getTrackAudioFeatures,
			pausePlayback,
			resumePlayback,
			playTrack,
			importTidalTrackForRadio,
			getAlbumTracks,
			replacePlaybackQueue,
			startRadioStart,
			getRadioTracks,
		},
		ApiError: class ApiError extends Error {},
	};
});

import { api } from '$lib/api/client';
import {
	currentQueueItemId,
	currentTrack,
	addTrackToQueue,
	isPlaying,
	lastSuccessfulCallAt,
	playAlbum,
	shuffleTidalTracksNow,
	playTidalTracksNow,
	startSongRadio,
	startTidalSongRadio,
	playTrackNow,
	playTidalTrackNow,
	togglePlayback,
	playbackQueue,
	playerError,
	refreshPlaybackState,
	moveQueueItem,
	removeTrackFromQueue,
	restoreQueueItems,
} from './player';

type AlbumTracksResult = Awaited<ReturnType<typeof api.getAlbumTracks>>;
type StartRadioResult = Awaited<ReturnType<typeof api.startRadioStart>>;
type RadioTracksResult = Awaited<ReturnType<typeof api.getRadioTracks>>;

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

/** A pending queue row on the unified mixed queue (unresolved TIDAL track). */
function pendingTidalRow(queueId: number, tidalId: number): QueueItem {
	return {
		id: queueId,
		position: queueId,
		source: 'radio_pending',
		is_pending: true,
		track: {
			id: 0,
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

/** A mixed-queue row already resolved: imported library track with a tidal id. */
function resolvedTidalRow(queueId: number, localId: number, tidalId: number): QueueItem {
	return {
		id: queueId,
		position: queueId,
		source: 'radio_pending',
		track: {
			id: localId,
			title: `Tidal ${tidalId}`,
			tidal_id: tidalId,
			source: 'tidal_stream',
		} as QueueItem['track'],
	};
}

type MixedQueueResponse = Awaited<ReturnType<typeof api.replacePlaybackQueue>>;

function mixedQueueResponse(current: QueueItem, queue: QueueItem[] = []): MixedQueueResponse {
	return {
		queued_count: queue.length + 1,
		pending_count: queue.filter((item) => item.is_pending).length,
		shuffle_debug: null,
		state: playbackState(current),
		queue,
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

function radioResults(trackIds: number[]): RadioTracksResult {
	return {
		tracks: trackIds.map((trackId) => ({
			track_id: trackId,
			title: `Radio ${trackId}`,
			artist_name: null,
			album_title: null,
			artwork_url: null,
			duration_ms: 200_000,
			best_quality: null,
			similarity_score: 1,
			adjusted_score: 1,
			co_listen_score: 0,
			co_album_score: 0,
			co_artist_score: 0,
			genre_proximity: 0,
			reason_tags: [],
			model_key: null,
			source_mode: 'engine',
		})),
		seed_track_id: trackIds[0] ?? 0,
		creativity: 0,
		context_window: 0,
		computed_at: null,
		model_family: null,
		model_key: null,
		reasons: [],
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

function tidalAlbumPlayable(index: number): TidalPlayable {
	const tidalId = 1000 + index;
	return {
		tidal_id: tidalId,
		title: `Anthology Track ${index}`,
		artist_name: 'The Beatles',
		artist_tidal_id: 3634161,
		album_title: 'Anthology 2',
		album_tidal_id: 58520793,
		artwork_url: `https://resources.tidal.com/images/test-${index}/640x640.jpg`,
		duration_ms: 180_000 + index,
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
		vi.mocked(api.getPlaybackState).mockReset();
		vi.mocked(api.removeQueueTrack).mockClear();
		vi.mocked(api.getTrackAudioFeatures).mockReset();
		vi.mocked(api.pausePlayback).mockReset();
		vi.mocked(api.resumePlayback).mockReset();
		vi.mocked(api.playTrack).mockReset();
		vi.mocked(api.importTidalTrackForRadio).mockReset();
		vi.mocked(api.getAlbumTracks).mockReset();
		vi.mocked(api.replacePlaybackQueue).mockReset();
		vi.mocked(api.startRadioStart).mockReset();
		vi.mocked(api.getRadioTracks).mockReset();
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
		vi.mocked(api.getTrackAudioFeatures).mockReset();
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

describe('moveQueueItem', () => {
	beforeEach(() => {
		vi.mocked(api.moveQueueTrack).mockClear();
		vi.mocked(api.getTrackAudioFeatures).mockReset();
		currentTrack.set(null);
		currentQueueItemId.set(null);
		isPlaying.set(false);
		playbackQueue.set([libraryRow(10, 1), libraryRow(20, 2)]);
	});

	it('applies playback state returned by the move endpoint', async () => {
		const current = libraryRow(10, 1);
		const next = libraryRow(20, 2);
		vi.mocked(api.moveQueueTrack).mockResolvedValueOnce({
			queue: [next, current],
			playback_state: playbackState(current),
		});

		await moveQueueItem(10, 1);

		expect(api.moveQueueTrack).toHaveBeenCalledWith(10, 1);
		expect(get(playbackQueue)).toEqual([next, current]);
		expect(get(currentTrack)?.id).toBe(1);
		expect(get(currentQueueItemId)).toBe(10);
		expect(get(isPlaying)).toBe(true);
		expect(api.getTrackAudioFeatures).toHaveBeenCalledWith(1);
	});
});

describe('stale playback responses', () => {
	beforeEach(() => {
		vi.mocked(api.getPlaybackState).mockReset();
		vi.mocked(api.getTrackAudioFeatures).mockReset();
		vi.mocked(api.playTrack).mockReset();
		vi.mocked(api.importTidalTrackForRadio).mockReset();
		vi.mocked(api.getAlbumTracks).mockReset();
		vi.mocked(api.replacePlaybackQueue).mockReset();
		vi.mocked(api.startRadioStart).mockReset();
		vi.mocked(api.getRadioTracks).mockReset();
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

	it('does not let an older add response erase a newer queue revision', async () => {
		const older = deferred<Awaited<ReturnType<typeof api.addQueueTrack>>>();
		const newer = deferred<Awaited<ReturnType<typeof api.addQueueTrack>>>();
		const first = libraryRow(10, 1);
		const second = libraryRow(20, 2);
		vi.mocked(api.addQueueTrack)
			.mockReturnValueOnce(older.promise)
			.mockReturnValueOnce(newer.promise);

		const olderAction = addTrackToQueue(1);
		const newerAction = addTrackToQueue(2);

		newer.resolve({ queue: [first, second], queue_revision: 12 });
		await newerAction;
		expect(get(playbackQueue).map((item) => item.id)).toEqual([10, 20]);

		older.resolve({ queue: [first], queue_revision: 11 });
		await olderAction;
		expect(get(playbackQueue).map((item) => item.id)).toEqual([10, 20]);
	});

	it('restores the transport button when a pause request fails', async () => {
		isPlaying.set(true);
		vi.mocked(api.pausePlayback).mockRejectedValueOnce(new Error('server not responding'));

		await togglePlayback();

		expect(get(isPlaying)).toBe(true);
		expect(get(playerError)?.message).toBeTruthy();
	});

	it('does not let an older album track fetch start a stale queue replace', async () => {
		const older = deferred<AlbumTracksResult>();
		const newer = deferred<AlbumTracksResult>();
		vi.mocked(api.getAlbumTracks)
			.mockReturnValueOnce(older.promise)
			.mockReturnValueOnce(newer.promise);
		vi.mocked(api.replacePlaybackQueue).mockImplementation(async (items) => {
			const queue = items.map((item, index) => libraryRow(index + 1, item.track_id ?? 0));
			return mixedQueueResponse(queue[0] ?? libraryRow(1, 0), queue);
		});
		vi.mocked(api.playTrack).mockImplementation(async (trackId: number) =>
			playbackSnapshot(libraryRow(trackId, trackId))
		);

		const olderAction = playAlbum(1);
		const newerAction = playAlbum(2);

		newer.resolve({ tracks: [libraryTrack(2)], tidal_tracks: [], album_tidal_id: null, album_is_favorite: false });
		await newerAction;
		expect(get(currentTrack)?.id).toBe(2);
		expect(api.replacePlaybackQueue).toHaveBeenCalledTimes(1);
		expect(vi.mocked(api.replacePlaybackQueue).mock.calls[0][0]).toEqual([{ track_id: 2, reason: null }]);

		older.resolve({ tracks: [libraryTrack(1)], tidal_tracks: [], album_tidal_id: null, album_is_favorite: false });
		await olderAction;
		expect(get(currentTrack)?.id).toBe(2);
		expect(api.replacePlaybackQueue).toHaveBeenCalledTimes(1);
	});

	it('ignores an older direct TIDAL play response after a newer TIDAL play', async () => {
		// Direct TIDAL playback now replaces the canonical queue, rather than
		// importing then calling the library-only play endpoint.
		const older = deferred<MixedQueueResponse>();
		const newer = deferred<MixedQueueResponse>();
		vi.mocked(api.replacePlaybackQueue)
			.mockReturnValueOnce(older.promise)
			.mockReturnValueOnce(newer.promise);

		const olderAction = playTidalTrackNow(tidalPlayable(1));
		const newerAction = playTidalTrackNow(tidalPlayable(2));

		newer.resolve(mixedQueueResponse(resolvedTidalRow(1, 502, 2)));
		await newerAction;
		expect(get(currentTrack)?.tidal_id).toBe(2);
		expect(get(currentTrack)?.id).toBe(502);

		older.resolve(mixedQueueResponse(resolvedTidalRow(1, 501, 1)));
		await olderAction;
		expect(get(currentTrack)?.tidal_id).toBe(2);
		expect(get(currentTrack)?.id).toBe(502);
		expect(get(playerError)).toBeNull();
	});

	it('hydrates TIDAL track-list playback from the server queue snapshot', async () => {
		const staleRow = libraryRow(90, 900);
		const current = resolvedTidalRow(1, 501, 101);
		const queued = pendingTidalRow(2, 102);
		playbackQueue.set([staleRow]);
		vi.mocked(api.replacePlaybackQueue).mockResolvedValueOnce(mixedQueueResponse(current, [queued]));

		await playTidalTracksNow([tidalPlayable(101), tidalPlayable(102)], 'album');

		expect(api.replacePlaybackQueue).toHaveBeenCalledTimes(1);
		expect(get(currentTrack)?.tidal_id).toBe(101);
		expect(get(playbackQueue).map((item) => item.track.tidal_id)).toEqual([102]);
		expect(get(playbackQueue).some((item) => item.track.id === 900)).toBe(false);
		expect(get(playerError)).toBeNull();
	});

	it('passes the full loaded TIDAL album track list to playback in order', async () => {
		const loadedAlbum = Array.from({ length: 45 }, (_, index) => tidalAlbumPlayable(index + 1));
		playbackQueue.set([libraryRow(90, 900)]);
		vi.mocked(api.replacePlaybackQueue).mockResolvedValueOnce(
			mixedQueueResponse(
				resolvedTidalRow(1, 501, loadedAlbum[0].tidal_id),
				loadedAlbum.slice(1).map((track, index) => pendingTidalRow(index + 2, track.tidal_id))
			)
		);

		await playTidalTracksNow(loadedAlbum, 'Anthology 2');

		expect(api.replacePlaybackQueue).toHaveBeenCalledTimes(1);
		const [sentItems, sentOptions] = vi.mocked(api.replacePlaybackQueue).mock.calls[0];
		expect(sentOptions).toEqual({ shuffleMode: undefined, startPlayback: true });
		expect(sentItems).toHaveLength(45);
		expect(sentItems.map((item) => item.tidal_id)).toEqual(
			loadedAlbum.map((track) => track.tidal_id)
		);
		expect(sentItems.every((item) => item.album_tidal_id === 58520793)).toBe(true);
		expect(sentItems.every((item) => item.artwork_url?.includes('resources.tidal.com'))).toBe(true);
		expect(get(currentTrack)?.tidal_id).toBe(loadedAlbum[0].tidal_id);
		expect(get(playbackQueue).map((item) => item.track.tidal_id)).toEqual(
			loadedAlbum.slice(1).map((track) => track.tidal_id)
		);
		expect(get(playbackQueue).some((item) => item.track.id === 900)).toBe(false);
		expect(get(playerError)).toBeNull();
	});

	it('passes the full loaded TIDAL album track list to shuffle playback in order', async () => {
		const loadedAlbum = Array.from({ length: 45 }, (_, index) => tidalAlbumPlayable(index + 1));
		vi.mocked(api.replacePlaybackQueue).mockResolvedValueOnce(
			mixedQueueResponse(
				resolvedTidalRow(1, 501, loadedAlbum[0].tidal_id),
				loadedAlbum.slice(1).map((track, index) => pendingTidalRow(index + 2, track.tidal_id))
			)
		);

		await shuffleTidalTracksNow(loadedAlbum, 'Anthology 2');

		expect(api.replacePlaybackQueue).toHaveBeenCalledTimes(1);
		const [sentItems, sentOptions] = vi.mocked(api.replacePlaybackQueue).mock.calls[0];
		expect(sentOptions).toEqual({ shuffleMode: 'true', startPlayback: true });
		expect(sentItems).toHaveLength(45);
		expect(sentItems.map((item) => item.tidal_id)).toEqual(
			loadedAlbum.map((track) => track.tidal_id)
		);
		expect(sentItems.every((item) => item.album_tidal_id === 58520793)).toBe(true);
		expect(sentItems.every((item) => item.artwork_url?.includes('resources.tidal.com'))).toBe(true);
		expect(get(playerError)).toBeNull();
	});

	it('ignores an older TIDAL track-list playback response after a newer one wins', async () => {
		const older = deferred<MixedQueueResponse>();
		const newer = deferred<MixedQueueResponse>();
		const olderCurrent = resolvedTidalRow(1, 501, 1);
		const newerCurrent = resolvedTidalRow(1, 502, 2);
		const newerQueued = pendingTidalRow(2, 3);
		vi.mocked(api.replacePlaybackQueue)
			.mockReturnValueOnce(older.promise)
			.mockReturnValueOnce(newer.promise);

		const olderAction = playTidalTracksNow([tidalPlayable(1)], 'older album');
		const newerAction = playTidalTracksNow([tidalPlayable(2), tidalPlayable(3)], 'newer album');

		newer.resolve(mixedQueueResponse(newerCurrent, [newerQueued]));
		await newerAction;
		expect(get(currentTrack)?.tidal_id).toBe(2);
		expect(get(playbackQueue).map((item) => item.track.tidal_id)).toEqual([3]);

		older.resolve(mixedQueueResponse(olderCurrent, [pendingTidalRow(2, 4)]));
		await olderAction;
		expect(get(currentTrack)?.tidal_id).toBe(2);
		expect(get(playbackQueue).map((item) => item.track.tidal_id)).toEqual([3]);
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

	it('ignores an older TIDAL radio lookup after a newer play request', async () => {
		const radio = deferred<RadioTracksResult>();
		const play = deferred<PlaybackSnapshot>();
		const currentRow = libraryRow(20, 2);
		vi.mocked(api.getRadioTracks).mockReturnValueOnce(radio.promise);
		vi.mocked(api.playTrack).mockReturnValueOnce(play.promise);
		vi.mocked(api.replacePlaybackQueue).mockResolvedValue(mixedQueueResponse(libraryRow(10, 1), [libraryRow(10, 1)]));

		const radioAction = startTidalSongRadio(tidalPlayable(1));
		const playAction = playTrackNow(2);

		play.resolve(playbackSnapshot(currentRow));
		await playAction;
		expect(get(currentTrack)?.id).toBe(2);

		radio.resolve(radioResults([1]));
		await radioAction;
		expect(get(currentTrack)?.id).toBe(2);
		expect(api.replacePlaybackQueue).not.toHaveBeenCalled();
		expect(get(playerError)).toBeNull();
	});
});
