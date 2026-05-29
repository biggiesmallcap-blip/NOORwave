import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'DailyChartShelf.svelte'), 'utf8');
const chartsPage = readFileSync(
	join(here, '..', '..', '..', 'routes', 'charts', '+page.svelte'),
	'utf8',
);

describe('daily chart shelf contract', () => {
	test('loads the additive snapshot endpoint without replacing trending or playlists', () => {
		expect(source).toContain('api.getChartSnapshot');
		expect(source).toContain('api.getChartMatrix');
		expect(source).toContain('api.refreshChartMatrix');
		expect(source).toContain("let selectedSource = $state('spotify_daily')");

		const trendingIndex = chartsPage.indexOf('<TrendingShelf limit={12} />');
		const dailyIndex = chartsPage.indexOf('<DailyChartShelf />');
		const playlistsIndex = chartsPage.indexOf('<h2 class="block-title">Spotify chart playlists</h2>');

		expect(trendingIndex).toBeGreaterThan(-1);
		expect(dailyIndex).toBeGreaterThan(trendingIndex);
		expect(playlistsIndex).toBeGreaterThan(dailyIndex);
	});

	test('keeps empty and restart states visible when refresh cannot populate data', () => {
		expect(source).toContain('No market snapshot yet');
		expect(source).toContain('No Spotify daily list for');
		expect(source).toContain('Global data is available above');
		expect(source).toContain('NOOR tried to refresh the provider matrix');
		expect(source).toContain('Daily snapshots unavailable');
		expect(source).toContain('Restart the NOOR server');
	});

	test('shows the full provider matrix target, not just Spotify', () => {
		expect(source).toContain('matrix.providers as provider');
		expect(source).toContain('{provider.label}');
		expect(source).toContain('row.cells[provider.source_key]');
		expect(source).toContain('matrixHasData(next)');
		expect(source).toContain('regionHasMatrixData(selectedRegion)');
	});

	test('makes region tabs and provider cards change the active provider snapshot', () => {
		expect(source).toContain('selectedRegionCells()');
		expect(source).toContain('pickProvider(selectedRegion, provider.source_key)');
		expect(source).toContain('loadSnapshot(region, source)');
		expect(source).toContain('Provider snapshot');
	});

	test('resolves visible provider leaders against TIDAL for artwork and playback', () => {
		expect(source).toContain('api.searchTidal(query, 1)');
		expect(source).toContain('resolvedTracks');
		expect(source).toContain('playTidalTrackNow');
		expect(source).toContain('TIDAL ready');
		expect(source).toContain('provider-card-art');
		expect(source).toContain('resolveVisibleEntries(entries)');
		expect(source).toContain('entryStatusLabel(entry)');
	});
});
