import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'DailyChartShelf.svelte'), 'utf8');
const muralSource = readFileSync(join(here, 'ChartMural.svelte'), 'utf8');
const chartsPage = readFileSync(
	join(here, '..', '..', '..', 'routes', 'charts', '+page.svelte'),
	'utf8',
);

describe('daily chart shelf contract', () => {
	test('loads chart snapshots and the provider matrix without replacing trending or playlists', () => {
		expect(source).toContain('api.getChartSnapshot');
		expect(source).toContain('api.getChartMatrix');
		expect(source).toContain('api.refreshChartMatrix');
		expect(source).toContain("let selectedSource = $state('spotify_daily')");
		expect(source).toContain('const LIMIT = 20');
		expect(source).toContain('snapshotRefreshAttempted');
		expect(source).toContain('next.entries.length < Math.min(10, LIMIT)');

		const trendingIndex = chartsPage.indexOf('<TrendingShelf limit={20} />');
		const dailyIndex = chartsPage.indexOf('<DailyChartShelf />');
		const playlistsIndex = chartsPage.indexOf('title="Chart playlists"');

		expect(trendingIndex).toBeGreaterThan(-1);
		expect(dailyIndex).toBeGreaterThan(trendingIndex);
		expect(playlistsIndex).toBeGreaterThan(dailyIndex);
		expect(chartsPage).toContain('PageHeader');
		expect(chartsPage).toContain('variant="editorial"');
		expect(chartsPage).toContain('SectionHeader');
		expect(chartsPage).toContain('variant="charts"');
		expect(chartsPage).toContain('level={2}');
		expect(chartsPage).not.toContain('style="background-image');
	});

	test('keeps empty and restart states visible when refresh cannot populate data', () => {
		expect(source).toContain('No market snapshot yet');
		expect(source).toContain('No provider leaders for');
		expect(source).toContain('Global data is available above');
		expect(source).toContain('NOOR tried to refresh the provider matrix');
		expect(source).toContain('Restart the NOOR server');
	});

	test('shows the full provider matrix target, not just Spotify', () => {
		expect(source).toContain('matrix.providers as provider');
		expect(source).toContain('{provider.label}');
		expect(source).toContain('row.cells[provider.source_key]');
		expect(source).toContain('matrixHasData(next)');
		expect(source).toContain('regionHasMatrixData(selectedRegion)');
	});

	test('uses provider chips and a top 20 mural instead of duplicate leader cards', () => {
		expect(source).toContain('SectionHeader');
		expect(source).toContain('variant="charts"');
		expect(source).toContain('level={2}');
		expect(source).toContain('source-tabs');
		expect(source).toContain('ChartMural');
		expect(source).toContain('type ChartMuralItem');
		expect(muralSource).toContain('chart-mural');
		expect(source).toContain('chartEntries.map((entry');
		expect(source).toContain('currentEntry');
		expect(source).toContain('pickProvider(selectedRegion, provider.source_key)');
		expect(source).not.toContain('provider-card');
		expect(source).not.toContain('Provider snapshot');
		expect(source).not.toContain('chart-mural-card');
	});

	test('resolves visible chart entries against TIDAL for artwork and playback', () => {
		expect(source).toContain('api.searchTidal(query, 1)');
		expect(source).toContain('resolvedTracks');
		expect(source).toContain('playTidalTrackNow');
		expect(source).toContain('TIDAL ready');
		expect(source).toContain("import { tidalSearchTrackToPlayable } from '$lib/utils/track';");
		expect(source).toContain('const playable = tidalSearchTrackToPlayable(hit);');
		expect(source).toContain('artwork_url: playable.artwork_url ?? fallbackArtwork');
		expect(source).not.toContain('artist_tidal_id: hit.artist_id');
		expect(muralSource).toContain('chart-mural-art');
		expect(source).toContain('async function resolveVisibleEntries(entries');
		expect(source).toContain('async function playEntry(entry');
	});

	test('uses shared context menus and standardized pill controls', () => {
		expect(source).toContain('openContextMenu');
		expect(source).toContain('buildTidalTrackMenu');
		expect(source).toContain('openEntryContext');
		expect(source).toContain('openMatrixCellContext');
		expect(source).toContain('oncontextmenu');
		expect(source).toContain('border: 1px solid var(--panel-border)');
		expect(source).toContain('border-color: var(--accent-line)');
	});

	test('keeps chart playlist card menus app-owned', () => {
		expect(chartsPage).toContain('function openChartPlaylistContext(e: MouseEvent');
		expect(chartsPage).toContain('e.preventDefault();');
		expect(chartsPage).toContain('e.stopPropagation();');
		expect(chartsPage).toContain('openContextMenu(e, chartMenu(chart.id, chart.title), chart.title);');
		expect(chartsPage).toContain('oncontextmenu={(e) => openChartPlaylistContext(e, c)}');
	});

	test('keeps chart page titles neutral with shared headers and artwork images', () => {
		expect(chartsPage).toContain('<PageHeader');
		expect(chartsPage).toContain('variant="editorial"');
		expect(chartsPage).toContain('<SectionHeader');
		expect(chartsPage).toContain('<ArtworkImage');
		expect(chartsPage).toContain('className="chart-playlist-art"');
		expect(chartsPage).not.toContain('service-spotify');
		expect(source).not.toContain('service-spotify');
		expect(muralSource).toContain('.chart-mural-title');
		expect(muralSource).toContain('color: var(--text-primary)');
	});

	test('loads playlist card covers through the metadata endpoint', () => {
		expect(chartsPage).toContain('api.getSpotifyPlaylistMeta(c.id, signal)');
		expect(chartsPage).not.toContain('api.getSpotifyPlaylist(c.id');
	});
});
