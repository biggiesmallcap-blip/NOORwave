import { describe, expect, test, vi } from 'vitest';

vi.mock('$lib/api/client', () => ({ api: {} }));
vi.mock('$lib/stores/player', () => ({ toggleTidalTrackFavorite: vi.fn() }));

import { pickSongForVideo, songTitleForVideo } from './like_song_for_video';
import type { TidalSearchTrack, TidalSearchVideo } from '$lib/api/client';

const video = (title: string, artist_name: string, artist_id: number | null = null): TidalSearchVideo => ({
	tidal_id: 1,
	title,
	duration_ms: null,
	artist_id,
	artist_name,
	album_tidal_id: null,
	artwork_url: null,
	quality: null,
	explicit: null,
	type: 'Music Video',
});

const track = (title: string, artist_name: string, artist_id: number | null = null): TidalSearchTrack => ({
	tidal_id: 2,
	title,
	duration_ms: 1,
	artist_id,
	artist_name,
	album_title: null,
	album_tidal_id: null,
	artwork_url: null,
	audio_quality: null,
	stream_ready: null,
	in_library: false,
});

describe('liking the song for a saved video', () => {
	test('drops video-only decorations but keeps featured artists', () => {
		expect(songTitleForVideo('The Funkiest (Official Video)')).toBe('The Funkiest');
		expect(songTitleForVideo('Alison (Video)')).toBe('Alison');
		expect(songTitleForVideo('Wicked Games (feat. Anna Naklab)')).toBe('Wicked Games (feat. Anna Naklab)');
	});

	test('picks the same title by the same artist', () => {
		const hits = [track('Fade Into You (Live)', 'Mazzy Star'), track('Fade Into You', 'Mazzy Star')];
		expect(pickSongForVideo(video('Fade Into You', 'Mazzy Star'), hits)?.title).toBe('Fade Into You');
	});

	test('refuses a cover by someone else', () => {
		expect(pickSongForVideo(video('Go', 'The Chemical Brothers'), [track('Go', 'Moby')])).toBeNull();
	});

	test('matches by TIDAL artist id even when the names differ', () => {
		expect(pickSongForVideo(video('Let Her Cry', 'Hootie & The Blowfish', 7), [track('Let Her Cry', 'Hootie And The Blowfish', 7)])).not.toBeNull();
	});
});
