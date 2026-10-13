import { beforeEach, describe, expect, test, vi } from 'vitest';
const mocks = vi.hoisted(() => ({ playQueue: vi.fn(), playTidal: vi.fn(), choose: vi.fn(), importTrack: vi.fn() }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
vi.mock('$lib/stores/player', () => ({ playQueueItemNow: mocks.playQueue, playTidalTrackNow: mocks.playTidal }));
vi.mock('$lib/stores/downloads', () => ({}));
vi.mock('$lib/stores/tidal', async () => ({ tidalStatus: (await import('svelte/store')).writable('connected') }));
vi.mock('$lib/stores/add_to_playlist', () => ({ choosePlaylist: mocks.choose }));
vi.mock('$lib/api/client', () => ({ api: { importTidalTrackForRadio: mocks.importTrack } }));
import { buildTrackMenu, buildTidalTrackMenu } from './track_menu';

const tidal = { tidal_id: 42, title: 'Song', artist_name: 'Artist', album_title: null, artwork_url: null, duration_ms: 1000 };
describe('shared track menus', () => {
	beforeEach(() => vi.clearAllMocks());
	test('local and TIDAL queue rows share core actions and never offer Add to queue', async () => {
		for (const menu of [buildTrackMenu({ id: 7, title: 'Song' }, { queueItemId: 5 }), buildTidalTrackMenu(tidal, { queueItemId: 5 })]) {
			const labels = menu.map((item) => item.label);
			for (const label of ['Play now', 'Move next', 'Add to playlist', 'Remove from queue']) expect(labels).toContain(label);
			expect(labels).not.toContain('Add to queue');
			await menu.find((item) => item.label === 'Play now')?.onSelect?.();
		}
		expect(mocks.playQueue).toHaveBeenCalledTimes(2);
		expect(mocks.playQueue).toHaveBeenCalledWith(5);
		expect(mocks.playTidal).not.toHaveBeenCalled();
	});
	test('playlist picker resolves a TIDAL track only after a destination is chosen', async () => {
		mocks.importTrack.mockResolvedValue({ local_id: 7 });
		buildTidalTrackMenu(tidal).find((item) => item.label === 'Add to playlist')?.onSelect?.();
		expect(mocks.choose).toHaveBeenCalledWith('Song', expect.any(Function));
		expect(mocks.importTrack).not.toHaveBeenCalled();
		expect(await mocks.choose.mock.calls[0][1]()).toEqual([7]);
	});
	test('TIDAL menus never stack or end on separators', () => {
		for (const options of [{}, { remoteRoutes: true }, { queueItemId: 5 }, { remoteRoutes: true, queueItemId: 5 }]) {
			const menu = buildTidalTrackMenu({ ...tidal, artist_tidal_id: 1, album_tidal_id: 2 }, options);
			const separators = menu.map((item) => item.separator === true);
			expect(separators.at(-1), JSON.stringify(options)).toBe(false);
			expect(separators.some((sep, i) => sep && separators[i + 1]), JSON.stringify(options)).toBe(false);
		}
	});
	test('unresolved queue rows still expose only the safe removal action', () => {
		expect(buildTrackMenu({ id: 0, title: 'Pending' }, { isPending: true, queueItemId: 5 }).map((item) => item.label)).toEqual(['Remove from queue']);
	});
});
