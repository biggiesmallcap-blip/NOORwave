import { describe, expect, test, vi } from 'vitest';

vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
vi.mock('$lib/stores/player', () => ({
	addTrackToQueue: vi.fn(),
	addTidalTrackToQueue: vi.fn(),
	moveQueueTrackNext: vi.fn(),
	playAlbum: vi.fn(),
	playArtist: vi.fn(),
	playTidalAlbum: vi.fn(),
	playTidalTrackNext: vi.fn(),
	playTidalTrackNow: vi.fn(),
	playTrackNext: vi.fn(),
	removeTrackFromQueue: vi.fn(),
	shuffleAlbum: vi.fn(),
	shuffleArtist: vi.fn(),
	startAlbumRadio: vi.fn(),
	startArtistRadio: vi.fn(),
	startSongRadio: vi.fn(),
	startTidalSongRadio: vi.fn(),
	toggleTrackFavorite: vi.fn(),
}));

import { moveQueueTrackNext } from '$lib/stores/player';
import { buildTidalTrackMenu, buildTrackMenu } from '../src/lib/player/track_menu';
import {
	albumRefFromTrack,
	artistRefFromTrack,
	buildMediaMenu,
	mediaHref,
	trackRefFromTrack,
} from '../src/lib/player/media_link';
import type { Track } from '../src/lib/api/client';

function localTrack(overrides: Partial<Track> = {}): Track {
	return {
		id: 12,
		title: 'Local Song',
		artist_id: 34,
		artist_name: 'Local Artist',
		artist_tidal_id: 3400,
		album_id: 56,
		album_title: 'Local Album',
		album_tidal_id: 5600,
		disc_number: null,
		track_number: null,
		duration_ms: 180000,
		isrc: null,
		tidal_id: 1200,
		best_quality: 'LOSSLESS',
		best_source: 'tidal',
		fidelity_score: 0,
		is_favorite: false,
		play_count: 0,
		last_played_at: null,
		date_added: null,
		source: 'library',
		artwork_url: null,
		...overrides,
	};
}

describe('canonical media link helpers', () => {
	test('normalizes local and TIDAL track metadata into stable hrefs', () => {
		const local = localTrack();
		expect(mediaHref(trackRefFromTrack(local))).toBe('/albums/56');
		expect(mediaHref(artistRefFromTrack(local))).toBe('/artists/34');
		expect(mediaHref(albumRefFromTrack(local))).toBe('/albums/56');

		const tidal = localTrack({
			id: -222,
			tidal_id: 222,
			artist_id: -1,
			artist_tidal_id: 333,
			album_id: null,
			album_tidal_id: 444,
		});

		expect(mediaHref(trackRefFromTrack(tidal))).toBeNull();
		expect(mediaHref(artistRefFromTrack(tidal))).toBe('/tidal/artists/333');
		expect(mediaHref(albumRefFromTrack(tidal))).toBe('/tidal/albums/444');
	});

	test('media refs delegate menus to shared builders', () => {
		const track = localTrack();
		expect(buildMediaMenu(trackRefFromTrack(track)).map((item) => item.label)).toContain('Add to queue');
		expect(buildMediaMenu(artistRefFromTrack(track)!).map((item) => item.label)).toContain('Open artist');
		expect(buildMediaMenu(albumRefFromTrack(track)!).map((item) => item.label)).toContain('Open album');
	});
});

describe('queue menu contracts', () => {
	test('local queue rows move existing items instead of adding duplicates', () => {
		const labels = buildTrackMenu(localTrack(), { queueItemId: 99 }).map((item) => item.label);

		expect(labels).toContain('Move next');
		expect(labels).not.toContain('Add to queue');

		const moveNext = buildTrackMenu(localTrack(), { queueItemId: 99 }).find((item) => item.label === 'Move next');
		moveNext?.onSelect?.();
		expect(moveQueueTrackNext).toHaveBeenCalledWith(99);
	});

	test('TIDAL queue rows hide duplicate add-to-queue actions', () => {
		const labels = buildTidalTrackMenu({
			tidal_id: 222,
			title: 'TIDAL Song',
			artist_name: 'TIDAL Artist',
			album_title: 'TIDAL Album',
			artwork_url: null,
			duration_ms: 180000,
			artist_tidal_id: 333,
			album_tidal_id: 444,
		}, { inQueue: true }).map((item) => item.label);

		expect(labels).toContain('Play next');
		expect(labels).not.toContain('Add to queue');
	});
});
