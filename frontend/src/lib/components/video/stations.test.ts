import { describe, expect, test } from 'vitest';
import type { VideoStationCard } from '$lib/api/client';
import { groupStations, spotlightArtistId } from './stations';

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
