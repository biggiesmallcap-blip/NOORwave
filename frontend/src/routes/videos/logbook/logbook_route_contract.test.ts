import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const page = readFileSync(join(here, '+page.svelte'), 'utf8');

describe('logbook route contract', () => {
	test('Liked, Saved and History are entries that link to their pages', () => {
		expect(page).toContain('href="/videos/liked"');
		expect(page).toContain('href="/videos/history"');
		expect(page).toContain('>Liked videos<');
		expect(page).toContain('>Saved<');
		expect(page).toContain('>History<');
	});

	test('counts come from cheap calls only', () => {
		expect(page).toContain('api.getSavedVideos()');
		expect(page).toContain('api.getVideoHistory(1)');
		expect(page).not.toContain('api.getLikedVideos(');
	});

	test('the census is plain numbers and per-scene coverage', () => {
		expect(page).toContain('api.getVideoCensus()');
		expect(page).toContain('censusFigures(census)');
		expect(page).toContain('api.getVideoAtlas()');
		expect(page).toContain('sceneCoverage(scenes)');
		expect(page).toContain('seenLabel(row)');
		expect(page).toContain('href={`/videos/archive?scene=${row.id}`}');
	});

	test('no badges or streaks', () => {
		expect(page).not.toMatch(/class="[^"]*(badge|streak)/i);
	});

	test('a census that is not built yet says so instead of failing', () => {
		expect(page).toContain("censusState = 'unavailable'");
		expect(page).toContain('The census appears here once the catalog has been sorted');
	});
});
