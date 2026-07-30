import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';
import { findAlbumMatch, findArtistMatch, normalizeCatalogName } from './recommendation_navigation';
import type {
	ProviderRecommendationItem,
	TidalSearchAlbum,
	TidalSearchArtist,
} from '$lib/api/client';

const artist = (name: string, id: number): TidalSearchArtist =>
	({ name, tidal_id: id }) as unknown as TidalSearchArtist;
const album = (title: string, artist_name: string, id: number): TidalSearchAlbum =>
	({ title, artist_name, tidal_id: id }) as unknown as TidalSearchAlbum;
const rec = (title: string, artist_name = ''): ProviderRecommendationItem =>
	({ title, artist_name }) as unknown as ProviderRecommendationItem;

const here = dirname(fileURLToPath(import.meta.url));
const rustModule = join(
	here,
	'../../../../../noor-server/src/db/catalog_name.rs',
);

/**
 * Catalogue names are folded in two places: here at click time, to pick a TIDAL
 * search result, and in Rust at resolve time, to match a local row. If the two
 * implementations disagree, a name resolves on one side and not the other and
 * nothing surfaces the mismatch until a user clicks the wrong thing.
 *
 * These cases are duplicated verbatim in NORMALIZE_PARITY_CASES in
 * noor-server/src/db/catalog_name.rs. The last test in this file checks the
 * Rust table still contains every input listed here, so deleting a case on one
 * side fails on the other.
 */
const PARITY_CASES: [input: string, expected: string][] = [
	// The failures that motivated this.
	['Sigur Rós', 'sigur ros'],
	['Beyoncé', 'beyonce'],
	['Tyler, The Creator', 'tyler the creator'],
	['Mötley Crüe', 'motley crue'],
	['Björk', 'bjork'],
	["Sinéad O'Connor", 'sinead o connor'],
	// Ampersand is substituted in place and adds no spacing of its own, so a
	// bare "AC&DC" runs together. The Rust port matches this rather than the
	// prettier reading, because divergence is the failure mode that matters.
	['Simon & Garfunkel', 'simon and garfunkel'],
	['Kruder & Dorfmeister', 'kruder and dorfmeister'],
	['AC&DC', 'acanddc'],
	['&', 'and'],
	// Punctuation runs collapse; leading and trailing ones vanish.
	['  The   Beatles  ', 'the beatles'],
	['Godspeed You! Black Emperor', 'godspeed you black emperor'],
	['Album (Deluxe Edition)', 'album deluxe edition'],
	['!!!', ''],
	['', ''],
	// Compatibility decomposition, which NFKD does and NFD would not.
	['ﬁnale', 'finale'],
	['Ｍｏｏｎ', 'moon'],
	// No canonical decomposition: these degrade to separators in both
	// languages. "Agaetis" would be the nicer fold but neither side produces
	// it, and matching each other is what counts.
	['Røyksopp', 'r yksopp'],
	['Ágætis byrjun', 'ag tis byrjun'],
	// Digits survive.
	['Sunn O)))', 'sunn o'],
	['2Pac', '2pac'],
];

describe('normalizeCatalogName', () => {
	test.each(PARITY_CASES)('folds %j to %j', (input, expected) => {
		expect(normalizeCatalogName(input)).toBe(expected);
	});

	test('folds names that differ only by accent or ampersand', () => {
		expect(normalizeCatalogName('Sigur Rós')).toBe(normalizeCatalogName('Sigur Ros'));
		expect(normalizeCatalogName('Simon & Garfunkel')).toBe(
			normalizeCatalogName('Simon and Garfunkel'),
		);
		expect(normalizeCatalogName('Tyler, The Creator')).toBe(
			normalizeCatalogName('Tyler The Creator'),
		);
	});

	test('keeps genuinely different names apart', () => {
		expect(normalizeCatalogName('The Beatles')).not.toBe(
			normalizeCatalogName('The Beatles Tribute'),
		);
		expect(normalizeCatalogName('Air')).not.toBe(normalizeCatalogName('Airs'));
	});

	test('handles null and undefined as empty', () => {
		expect(normalizeCatalogName(null)).toBe('');
		expect(normalizeCatalogName(undefined)).toBe('');
	});

	test('is idempotent', () => {
		for (const [input] of PARITY_CASES) {
			const once = normalizeCatalogName(input);
			expect(normalizeCatalogName(once)).toBe(once);
		}
	});

	test('folds every non-ASCII-alphanumeric name to the same empty string', () => {
		// Not a defect in the fold itself - it is ASCII-only by design, and the
		// Rust port agrees. It is why the matchers below cannot compare on the
		// fold alone, and why the server refuses an empty fold in SQL.
		expect(normalizeCatalogName('Грибы')).toBe('');
		expect(normalizeCatalogName('鄧麗君')).toBe('');
	});

	test('the Rust port pins the same inputs', () => {
		const rust = readFileSync(rustModule, 'utf8');
		const table = rust.slice(
			rust.indexOf('NORMALIZE_PARITY_CASES'),
			rust.indexOf('#[cfg(test)]\nmod tests'),
		);
		expect(table.length).toBeGreaterThan(0);
		for (const [input] of PARITY_CASES) {
			if (input === '') continue; // the empty case is not greppable
			expect(table, `Rust parity table is missing ${JSON.stringify(input)}`).toContain(input);
		}
	});
});

describe('non-Latin names do not collide on the empty fold', () => {
	// Both names fold to '', so comparing on the fold alone made every
	// non-Latin name equal to every other one and the matcher opened whichever
	// happened to come back first.
	test('findArtistMatch refuses an unrelated non-Latin artist', () => {
		expect(
			findArtistMatch(rec('Грибы'), [artist('鄧麗君', 1), artist('Вектор А', 2)]),
		).toBeNull();
	});

	test('findArtistMatch still matches a non-Latin artist against itself', () => {
		const wanted = artist('Грибы', 3);
		expect(findArtistMatch(rec('Грибы'), [artist('鄧麗君', 1), wanted])).toBe(wanted);
	});

	test('findAlbumMatch does not treat a non-Latin artist as "artist unknown"', () => {
		// The empty fold used to make `!wantedArtist` true, which skipped the
		// artist check entirely and let any album through on title alone.
		expect(
			findAlbumMatch(rec('黑膠', 'Грибы'), [album('黑膠', '鄧麗君', 1)]),
		).toBeNull();
	});

	test('findAlbumMatch still matches a non-Latin album by the same artist', () => {
		const wanted = album('黑膠', 'Грибы', 2);
		expect(findAlbumMatch(rec('黑膠', 'Грибы'), [album('黑膠', '鄧麗君', 1), wanted])).toBe(
			wanted,
		);
	});

	// A sole album by the wanted artist used to be accepted without looking at
	// the title at all, which opened TIDAL's one Althea & Donna album under two
	// different Last.fm titles.
	test('findAlbumMatch refuses the only album by the artist when the title is unrelated', () => {
		expect(
			findAlbumMatch(rec('Uptown Top Ranking', 'Althea & Donna'), [
				album('Hurt So Good', 'Althea & Donna', 1),
			]),
		).toBeNull();
	});

	test('findAlbumMatch still accepts an edition suffix on the same album', () => {
		const wanted = album('Hurt So Good', 'Althea & Donna', 1);
		expect(
			findAlbumMatch(rec('Hurt So Good (Bonus Track Edition)', 'Althea & Donna'), [wanted]),
		).toBe(wanted);
	});
});
