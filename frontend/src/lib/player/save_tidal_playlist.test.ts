import { beforeEach, describe, expect, test, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
	importTrack: vi.fn(), createPlaylist: vi.fn(), invalidate: vi.fn(),
}));
vi.mock('$lib/api/client', () => ({ api: {
	importTidalTrackForRadio: mocks.importTrack,
	createTidalPlaylist: mocks.createPlaylist,
} }));
vi.mock('$lib/cache/ws_events', () => ({ invalidatePlaylistCaches: mocks.invalidate }));
import { saveTidalPlaylist } from './save_tidal_playlist';

const track = { tidal_id: 42, title: 'One', artist_name: 'Artist', album_title: null,
	artwork_url: null, duration_ms: 1000 };

describe('save the displayed TIDAL collection', () => {
	beforeEach(() => { vi.resetAllMocks(); });
	test('imports once per TIDAL id and saves the exact order including repeats', async () => {
		mocks.importTrack.mockResolvedValue({ local_id: 7 });
		mocks.createPlaylist.mockResolvedValue({ playlist: { id: 3, tidal_uuid: 'saved' } });
		const result = await saveTidalPlaylist(' Mix ', [track, track]);
		expect(mocks.importTrack).toHaveBeenCalledTimes(1);
		expect(mocks.createPlaylist).toHaveBeenCalledWith('Mix', [7, 7], [42, 42]);
		expect(result.tidal_uuid).toBe('saved');
		expect(mocks.invalidate).toHaveBeenCalledOnce();
	});
	test('does not create an empty or partly imported playlist', async () => {
		await expect(saveTidalPlaylist('Mix', [])).rejects.toThrow();
		mocks.importTrack.mockRejectedValue(new Error('Import failed'));
		await expect(saveTidalPlaylist('Mix', [track])).rejects.toThrow('Import failed');
		expect(mocks.createPlaylist).not.toHaveBeenCalled();
	});
	test('surfaces a remote save failure without invalidating as a success', async () => {
		mocks.importTrack.mockResolvedValue({ local_id: 7 });
		mocks.createPlaylist.mockRejectedValue(new Error('TIDAL unavailable'));
		await expect(saveTidalPlaylist('Mix', [track])).rejects.toThrow('TIDAL unavailable');
		expect(mocks.invalidate).not.toHaveBeenCalled();
	});
});
