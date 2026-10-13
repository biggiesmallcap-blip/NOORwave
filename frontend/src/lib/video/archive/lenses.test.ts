import { describe, expect, test } from 'vitest';
import {
	DEFAULT_LENSES,
	activeLensCount,
	chipCount,
	clearLenses,
	eraLabel,
	lensSummary,
	lensesToQuery,
	lensesToSearch,
	parseLenses,
	toggleLens,
	type ArchiveLenses,
} from './lenses';

const parse = (search: string) => parseLenses(new URLSearchParams(search));

describe('archive lenses <-> URL', () => {
	test('an empty query is the default lens state and an empty search', () => {
		expect(parse('')).toEqual(DEFAULT_LENSES);
		expect(lensesToSearch(DEFAULT_LENSES)).toBe('');
	});

	test('an Atlas deep link opens on its scene', () => {
		expect(parse('?scene=12')).toEqual({ ...DEFAULT_LENSES, scene: 12 });
	});

	test('every lens round-trips through the query string', () => {
		const lenses: ArchiveLenses = {
			scene: 7,
			cut: 'live',
			eraFrom: 1980,
			eraTo: 1999,
			depth: 'deep',
			length: 'long',
			source: 'liked',
			sort: 'versions',
			q: 'swear it again',
		};
		const search = lensesToSearch(lenses);
		expect(search).toBe('?scene=7&cut=live&era_from=1980&era_to=1999&depth=deep&length=long&source=liked&sort=versions&q=swear+it+again');
		expect(parse(search)).toEqual(lenses);
	});

	test('defaults stay out of the URL so equal states give equal links', () => {
		expect(lensesToSearch({ ...DEFAULT_LENSES, source: 'catalog', sort: 'relevance' })).toBe('');
		expect(lensesToSearch({ ...DEFAULT_LENSES, sort: 'year' })).toBe('?sort=year');
	});

	test('malformed values fall back instead of breaking the page', () => {
		const lenses = parse('?scene=abc&cut=bogus&era_from=12&depth=huge&source=x&sort=y&length=');
		expect(lenses).toEqual(DEFAULT_LENSES);
	});

	test('a reversed era range is put the right way round', () => {
		expect(parse('?era_from=1999&era_to=1980')).toMatchObject({ eraFrom: 1980, eraTo: 1999 });
	});
});

describe('archive lenses -> API query', () => {
	test('carries only the active lenses, with paging', () => {
		expect(lensesToQuery(DEFAULT_LENSES)).toEqual({ sort: 'relevance', offset: 0, limit: 60 });
		expect(lensesToQuery({ ...DEFAULT_LENSES, scene: 3, cut: 'session', eraFrom: 1990, eraTo: 1999, source: 'saved' }, 120, 30)).toEqual({
			sort: 'relevance',
			offset: 120,
			limit: 30,
			scene: 3,
			cut: 'session',
			era_from: 1990,
			era_to: 1999,
			source: 'saved',
		});
	});
});

describe('lens helpers', () => {
	test('toggling picks a value and picking it again puts it down', () => {
		const once = toggleLens(DEFAULT_LENSES, 'cut', 'live');
		expect(once.cut).toBe('live');
		expect(toggleLens(once, 'cut', 'live').cut).toBeNull();
		expect(toggleLens(once, 'cut', 'official').cut).toBe('official');
	});

	test('active count ignores sort and the default source; clear keeps the sort', () => {
		const lenses: ArchiveLenses = { ...DEFAULT_LENSES, scene: 1, eraFrom: 1980, eraTo: 1989, sort: 'year' };
		expect(activeLensCount(lenses)).toBe(2);
		expect(clearLenses(lenses)).toEqual({ ...DEFAULT_LENSES, sort: 'year' });
	});

	test('era labels read as decades when aligned', () => {
		expect(eraLabel(null, null)).toBe('Any year');
		expect(eraLabel(1980, 1989)).toBe('1980s');
		expect(eraLabel(1980, 1999)).toBe('1980s - 1990s');
		expect(eraLabel(1994, 2003)).toBe('1994 - 2003');
	});

	test('summary names the lenses for the player source label', () => {
		expect(lensSummary(DEFAULT_LENSES, null)).toBe('Archive');
		expect(lensSummary({ ...DEFAULT_LENSES, scene: 4, cut: 'live' }, 'K-pop')).toBe('Archive: K-pop, Live');
	});

	test('chip counts stay short', () => {
		expect(chipCount(840)).toBe('840');
		expect(chipCount(1240)).toBe('1.2k');
		expect(chipCount(12400)).toBe('12k');
		expect(chipCount(1_500_000)).toBe('1.5M');
	});
});
