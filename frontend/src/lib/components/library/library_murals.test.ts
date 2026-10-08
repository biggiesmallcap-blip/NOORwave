import { describe, expect, test } from 'vitest';
import type { Track } from '$lib/api/client';
import {
	HOME_MURAL_ITEM_LIMIT,
	HOME_PANEL_CACHE_REFRESH_MS,
	buildMuralPanels,
	capPerArtist,
	fallbackLetters,
	homePanelRefreshBucket,
	muralItemKey,
	muralItemLazyQuery,
	panelQueueTrackIds,
	trackToMuralItem,
	uniqueById,
	type HomeAlbumCard,
} from './library_murals';

function track(id: number, artistId: number, overrides: Partial<Track> = {}): Track {
	return {
		id,
		title: `Track ${id}`,
		artist_id: artistId,
		artist_name: `Artist ${artistId}`,
		album_id: null,
		album_title: null,
		artwork_url: null,
		...overrides,
	} as Track;
}

function album(id: number): HomeAlbumCard {
	return { id, title: `Album ${id}`, artist_id: id, artist_name: `Artist ${id}`, artwork_url: null };
}

describe('capPerArtist', () => {
	test('caps each artist then tops up from the skipped tracks', () => {
		const input = [track(1, 1), track(2, 1), track(3, 1), track(4, 1), track(5, 2)];
		const out = capPerArtist(input, 2, 5).map((t) => t.id);
		// Head is shaped by the cap (1, 2, then artist 2), the tail is topped up
		// so the panel is not left short.
		expect(out).toEqual([1, 2, 5, 3, 4]);
	});

	test('respects the limit', () => {
		const input = Array.from({ length: 30 }, (_, i) => track(i + 1, i + 1));
		expect(capPerArtist(input, 2, HOME_MURAL_ITEM_LIMIT)).toHaveLength(HOME_MURAL_ITEM_LIMIT);
	});

	test('never caps tracks without an artist key', () => {
		const input = [1, 2, 3, 4].map((id) => track(id, 0, { artist_id: undefined as unknown as number, artist_name: null }));
		expect(capPerArtist(input, 1, 4).map((t) => t.id)).toEqual([1, 2, 3, 4]);
	});
});

describe('buildMuralPanels', () => {
	test('keeps display order and drops empty panels', () => {
		const panels = buildMuralPanels({
			suggestionTracks: [track(1, 1)],
			suggestionAlbums: [],
			randomTracks: [],
			randomAlbums: [album(9)],
		});
		expect(panels.map((p) => p.id)).toEqual(['suggested-tracks', 'random-albums']);
		expect(panels[0].caption).toBe('Listen history suggestions');
		expect(panels[1].caption).toBe('Library shuffle picks');
	});

	test('caps every panel at the item limit', () => {
		const many = Array.from({ length: 40 }, (_, i) => track(i + 1, i + 1));
		const albums = Array.from({ length: 40 }, (_, i) => album(i + 1));
		const panels = buildMuralPanels({ suggestionTracks: many, suggestionAlbums: albums, randomTracks: many, randomAlbums: albums });
		for (const panel of panels) expect(panel.items.length).toBe(HOME_MURAL_ITEM_LIMIT);
	});
});

describe('panelQueueTrackIds', () => {
	test('queues the panel in order without duplicates', () => {
		const [panel] = buildMuralPanels({
			suggestionTracks: [track(3, 1), track(5, 2), track(3, 1), track(7, 3)],
			suggestionAlbums: [],
			randomTracks: [],
			randomAlbums: [],
		});
		expect(panelQueueTrackIds(panel, 5)).toEqual([3, 5, 7]);
	});

	test('puts a clicked track that is not in the panel first', () => {
		const [panel] = buildMuralPanels({ suggestionTracks: [track(3, 1)], suggestionAlbums: [], randomTracks: [], randomAlbums: [] });
		expect(panelQueueTrackIds(panel, 99)).toEqual([99, 3]);
	});
});

describe('helpers', () => {
	test('keys never collide between tracks and albums with the same id', () => {
		const t = trackToMuralItem(track(5, 1));
		expect(muralItemKey(t)).toBe('track-5');
		const [albumPanel] = buildMuralPanels({ suggestionTracks: [], suggestionAlbums: [album(5)], randomTracks: [], randomAlbums: [] });
		expect(muralItemKey(albumPanel.items[0])).toBe('album-5');
	});

	test('lazy queries use the album or track artist', () => {
		const t = trackToMuralItem(track(5, 1));
		expect(muralItemLazyQuery(t)).toEqual({ artist: 'Artist 1', title: 'Track 5' });
	});

	test('fallback letters and dedupe', () => {
		expect(fallbackLetters('Random albums')).toBe('RA');
		expect(fallbackLetters('')).toBe('?');
		expect(uniqueById([{ id: 1 }, { id: 1 }, { id: 2 }]).map((x) => x.id)).toEqual([1, 2]);
	});

	test('refresh bucket changes every five minutes', () => {
		expect(homePanelRefreshBucket(0)).toBe(0);
		expect(homePanelRefreshBucket(HOME_PANEL_CACHE_REFRESH_MS - 1)).toBe(0);
		expect(homePanelRefreshBucket(HOME_PANEL_CACHE_REFRESH_MS)).toBe(1);
	});
});
