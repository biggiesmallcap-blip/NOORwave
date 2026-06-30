import { describe, it, expect } from 'vitest';
import { searchSettings, SETTINGS_SEARCH_INDEX } from './settingsSearch';

describe('searchSettings', () => {
	it('returns nothing for an empty query', () => {
		expect(searchSettings('')).toEqual([]);
		expect(searchSettings('   ')).toEqual([]);
	});

	it('finds a section by a control keyword, not just its title', () => {
		const ids = searchSettings('exclusive').map((e) => e.id);
		expect(ids).toContain('playback-output');
	});

	it('matches synonyms (scrobble -> Last.fm)', () => {
		const ids = searchSettings('scrobble').map((e) => e.id);
		expect(ids).toContain('last-fm-tags');
	});

	it('requires every term to match (AND semantics)', () => {
		expect(searchSettings('download flac').map((e) => e.id)).toContain('downloads');
		expect(searchSettings('download wallpaper')).toEqual([]);
	});

	it('every entry id is unique', () => {
		const ids = SETTINGS_SEARCH_INDEX.map((e) => e.id);
		expect(new Set(ids).size).toBe(ids.length);
	});
});
