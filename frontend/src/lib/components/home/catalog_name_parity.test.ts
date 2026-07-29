import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';
import { normalizeCatalogName } from './recommendation_navigation';

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
