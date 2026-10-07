import { describe, expect, test } from 'vitest';
import type { VideoStationCard } from '$lib/api/client';
import { cleanBio, groupStations, numberStations, previewArtists, sceneFamilyLabel, spotlightArtistId, stationFrames, stationMeta } from './stations';

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

describe('channel guide', () => {
	test('frames are the previews that have artwork, in order', () => {
		const station = withArtists(card('wild-card', 'for_you'), '', ['A', 'B', 'C']);
		station.preview[0].artwork_url = 'https://img/a';
		station.preview[2].artwork_url = 'https://img/c';
		expect(stationFrames(station).map((video) => video.tidal_id)).toEqual([1, 3]);
	});

	test('numbers channels down the page, spotlight first', () => {
		const { spotlight, rows } = groupStations([
			card('genre:rock', 'genres'),
			card('spotlight:5396', 'spotlight'),
			card('wild-card', 'for_you'),
			card('shuffle', 'for_you'),
			card('charts', 'charts'),
		]);
		const numbers = numberStations(spotlight, rows);
		expect([...numbers.entries()]).toEqual([
			['spotlight:5396', '01'],
			['wild-card', '02'],
			['shuffle', '03'],
			['genre:rock', '04'],
			['charts', '05'],
		]);
		expect(numberStations(null, rows).get('wild-card')).toBe('01');
	});
});

describe('explore scenes', () => {
	test('sit between vibes and themes', () => {
		const { rows } = groupStations([card('live', 'themes'), card('scene:jazz', 'explore'), card('vibe:dark', 'vibes')]);
		expect(rows.map((row) => row.id)).toEqual(['vibes', 'explore', 'themes']);
	});

	test('label a family once, where it starts', () => {
		const row = ['scene:latin', 'scene:reggaeton', 'scene:metal', 'scene:punk', 'scene:jazz'].map((id) => card(id, 'explore'));
		expect(row.map((_, index) => sceneFamilyLabel(row, index))).toEqual([
			'Latin and Caribbean', null, 'Heavier', null, 'Timeless',
		]);
		expect(sceneFamilyLabel([card('genre:pop', 'genres')], 0)).toBeNull();
	});
});

describe('spotlight bio cleanup', () => {
	const long = 'Leonard Cohen was a Canadian singer, songwriter, poet and novelist.';

	test('unwraps TIDAL link markup to its text', () => {
		expect(cleanBio(`[wimpLink artistId="3829"]Leonard Cohen[/wimpLink] was a Canadian singer, songwriter, poet and novelist.`))
			.toBe(long);
		expect(cleanBio(`He toured with [wimpLink albumId="12"]Songs of Love and Hate[/wimpLink] and [b]Various Positions[/b] in the eighties.`))
			.toBe('He toured with Songs of Love and Hate and Various Positions in the eighties.');
	});

	test('drops tags, decodes entities and collapses whitespace', () => {
		expect(cleanBio('Simon &amp; Garfunkel were a folk duo<br/><br />from   New York&#44; and they&#x27;re &quot;sung&quot;.'))
			.toBe(`Simon & Garfunkel were a folk duo from New York, and they're "sung".`);
	});

	test('keeps only the first sentence', () => {
		expect(cleanBio(`${long} His songs explored faith, love, loss and politics across six decades.`)).toBe(long);
		expect(cleanBio('Produced by Dr. Dre and J. Cole, the record went on to sell millions. It was later remastered.'))
			.toBe('Produced by Dr. Dre and J. Cole, the record went on to sell millions.');
	});

	test('a long first sentence ends at a clause break, never mid-word', () => {
		const cohen = "One of the most fascinating and enigmatic - if not the most successful - singer/songwriters of the late '60s, "
			+ 'Leonard Cohen retained an audience across six decades of music-making, interrupted by various digressions into personal and spiritual exile.';
		const cut = cleanBio(cohen)!;
		expect(cut).toBe("One of the most fascinating and enigmatic - if not the most successful - singer/songwriters of the late '60s, "
			+ 'Leonard Cohen retained an audience across six decades of music-making...');
		const words = cleanBio(`${'word '.repeat(80)}end`)!;
		expect(words.endsWith('word...')).toBe(true);
		expect(words.length).toBeLessThanOrEqual(203);
	});

	test('hides bios that are empty, too short or still broken', () => {
		expect(cleanBio(null)).toBeNull();
		expect(cleanBio('   ')).toBeNull();
		expect(cleanBio('Singer.')).toBeNull();
		expect(cleanBio('[wimpLink artistId="1"]Leonard Cohen was a Canadian singer and a poet of rare and stubborn grace')).toBeNull();
	});
});
