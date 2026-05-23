import { describe, expect, it, vi, beforeEach } from 'vitest';
import type { QueueItem } from '$lib/api/client';

// Mock the api client so we can drive restoreQueueItems without a server.
// vi.mock is hoisted, so the factory has to declare its own spies, then we
// re-import them from the module to assert against.
vi.mock('$lib/api/client', async () => {
	const addQueueTrack = vi.fn(async () => ({ queue: [], playback_state: null }));
	const queueAppend = vi.fn(async () => ({ queue: [], playback_state: null }));
	const getPlaybackState = vi.fn(async () => ({ queue: [] }));
	return {
		api: { addQueueTrack, queueAppend, getPlaybackState },
		ApiError: class ApiError extends Error {},
	};
});

import { api } from '$lib/api/client';
import { restoreQueueItems } from './player';

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

describe('restoreQueueItems', () => {
	beforeEach(() => {
		vi.mocked(api.addQueueTrack).mockClear();
		vi.mocked(api.queueAppend).mockClear();
		vi.mocked(api.getPlaybackState).mockClear();
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
