import { describe, expect, test } from 'vitest';
import type { TidalDiscographyTrack, Track } from '$lib/api/client';
import { artistCurrentTrackMatchesArtist } from './artist_playback';

function track(overrides: Partial<Track>): Track {
	return {
		id: 1,
		title: 'Track',
		artist_id: 10,
		artist_name: 'Artist',
		artist_tidal_id: null,
		album_id: null,
		album_title: null,
		album_tidal_id: null,
		disc_number: null,
		track_number: null,
		duration_ms: null,
		isrc: null,
		tidal_id: null,
		best_quality: null,
		best_source: null,
		fidelity_score: 0,
		is_favorite: false,
		play_count: 0,
		last_played_at: null,
		date_added: null,
		source: 'tidal_stream',
		artwork_url: null,
		...overrides,
	};
}

function tidalTrack(tidalId: number): TidalDiscographyTrack {
	return {
		tidal_id: tidalId,
		title: 'TIDAL Track',
		duration_ms: 200_000,
		artwork_url: null,
		album_title: null,
	};
}

describe('artistCurrentTrackMatchesArtist', () => {
	test('matches local artist tracks by local track id', () => {
		const current = track({ id: 42 });
		const local = [track({ id: 42 })];

		expect(artistCurrentTrackMatchesArtist(current, local, null, [])).toBe(true);
	});

	test('matches TIDAL fallback playback by artist TIDAL id', () => {
		const current = track({ id: -101, tidal_id: 101, artist_tidal_id: 6648 });

		expect(artistCurrentTrackMatchesArtist(current, [], 6648, [])).toBe(true);
	});

	test('matches TIDAL fallback playback by top-track TIDAL id when artist metadata is missing', () => {
		const current = track({ id: -101, tidal_id: 101, artist_tidal_id: null });

		expect(artistCurrentTrackMatchesArtist(current, [], 6648, [tidalTrack(101)])).toBe(true);
	});

	test('does not match unrelated TIDAL playback', () => {
		const current = track({ id: -202, tidal_id: 202, artist_tidal_id: 9999 });

		expect(artistCurrentTrackMatchesArtist(current, [], 6648, [tidalTrack(101)])).toBe(false);
	});
});
