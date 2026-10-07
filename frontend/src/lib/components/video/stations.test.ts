import { describe, expect, test } from 'vitest';
import type { VideoStationCard } from '$lib/api/client';
import { groupStations, previewArtists, spotlightArtistId, stationMeta } from './stations';

function card(id: string, group: VideoStationCard['group']): VideoStationCard {
	return { id, group, title: id, subtitle: '', unwatched_count: 40, preview: [] };
}

describe('station grouping', () => {
	test('splits out the spotlight and keeps row order, hiding empty rows', () => {
		const { spotlight, rows } = groupStations([
			card('genre:rock', 'genres'),
			card('spotlight:5396', 'spotlight'),
			card('wild-card', 'for_you'),
			card('charts', 'charts'),
		]);
		expect(spotlight?.id).toBe('spotlight:5396');
		expect(rows.map((row) => row.id)).toEqual(['for_you', 'genres', 'charts']);
		expect(rows[0].title).toBe('For you');
	});

	test('reads the spotlight artist id from the station id', () => {
		expect(spotlightArtistId(card('spotlight:5396', 'spotlight'))).toBe(5396);
		expect(spotlightArtistId(card('wild-card', 'for_you'))).toBeNull();
	});
});

function withArtists(base: VideoStationCard, subtitle: string, artists: (string | null)[]): VideoStationCard {
	return {
		...base,
		subtitle,
		preview: artists.map((artist_name, index) => ({
			tidal_id: index + 1, title: `Song ${index}`, duration_ms: null, artist_id: null,
			artist_name, album_tidal_id: null, artwork_url: null, quality: null, explicit: null,
			type: 'Music Video',
		})),
	};
}

describe('station meta line', () => {
	test('lists distinct preview artists, skipping blanks', () => {
		const station = withArtists(card('genre:rock', 'genres'), '', ['Elvis Presley', 'elvis presley', null, 'Bob Dylan', 'Moby', 'Otis Redding']);
		expect(previewArtists(station)).toEqual(['Elvis Presley', 'Bob Dylan', 'Moby']);
	});

	test('keeps a subtitle only when no other card in the row shares it', () => {
		const shared = 'Artists you like and ones you might';
		const rock = withArtists(card('genre:rock', 'genres'), shared, ['Elvis Presley', 'Bob Dylan']);
		const pop = withArtists(card('genre:pop', 'genres'), shared, ['Moby']);
		const dance = withArtists(card('vibe:dance', 'vibes'), 'Dance, club and party', ['Moby']);
		expect(stationMeta(rock, [rock, pop])).toBe('Elvis Presley, Bob Dylan');
		expect(stationMeta(dance, [dance, rock])).toBe('Dance, club and party');
	});

	test('falls back to the video count when there is nothing else to say', () => {
		const bare = withArtists(card('charts', 'charts'), '', []);
		expect(stationMeta(bare, [bare])).toBe('40 videos you have not seen');
	});
});
