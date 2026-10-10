import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'DailyChartShelf.svelte'), 'utf8');
const muralSource = readFileSync(join(here, 'ChartMural.svelte'), 'utf8');
const matchSource = readFileSync(join(here, 'chart_tidal_match.ts'), 'utf8');
const queriesSource = readFileSync(join(here, '..', '..', 'cache', 'api_queries.ts'), 'utf8');
const chartsPage = readFileSync(
	join(here, '..', '..', '..', 'routes', 'charts', '+page.svelte'),
	'utf8',
);

describe('daily chart shelf contract', () => {
	test('loads chart snapshots and the provider matrix without replacing trending or playlists', () => {
		expect(source).toContain('cachedApi.chartSnapshotQuery(');
		expect(source).toContain('cachedApi.chartMatrixQuery()');
		expect(source).toContain('api.refreshChartMatrix');
		expect(source).toContain("selectedChartSource");
		expect(source).toContain('const LIMIT = 20');
		expect(source).toContain('snapshotRefreshAttempted');
		expect(source).toContain('state.data!.entries.length < Math.min(10, LIMIT)');

		const trendingIndex = chartsPage.indexOf('<TrendingShelf limit={20} />');
		const dailyIndex = chartsPage.indexOf('<DailyChartShelf />');
		const playlistsIndex = chartsPage.indexOf('title="Chart playlists"');

		expect(trendingIndex).toBeGreaterThan(-1);
		expect(dailyIndex).toBeGreaterThan(trendingIndex);
		expect(playlistsIndex).toBeGreaterThan(dailyIndex);
		expect(chartsPage).toContain('PageHeader');
		expect(chartsPage).not.toContain('variant="editorial"');
		expect(chartsPage).toContain('SectionHeader');
		expect(chartsPage).toContain('variant="charts"');
		expect(chartsPage).toContain('level={2}');
		expect(chartsPage).not.toContain('style="background-image');
	});

	test('caches chart payloads for instant paint and background revalidation', () => {
		expect(queriesSource).toContain("charts: {");
		expect(queriesSource).toContain("['api', 'charts', 'snapshot', opts]");
		expect(queriesSource).toContain("['api', 'charts', 'matrix']");
		expect(queriesSource).toContain('persist: scopedPersist(DAY)');
		expect(queriesSource).toContain('returnStale: true');
		// An empty answer must not pin an empty shelf for the whole window.
		expect(queriesSource).toContain('rows.length > 0 ? 15 * MINUTE : 30 * SECOND');
		expect(queriesSource).toContain('export function invalidateChartSnapshotCaches()');
		expect(source).toContain('invalidateChartSnapshotCaches();');
	});

	test('refreshes a matrix that is empty or behind today, once per session', () => {
		expect(source).toContain('<script module lang="ts">');
		expect(source).toContain('let matrixRefreshStarted = false;');
		// When a matrix is stale is behaviour-tested in chart_rules.test.ts.
		expect(source).toContain("from './chart_rules'");
		expect(source).toContain('!state.loading && !state.refreshing && matrixNeedsRefresh(state.data)');
	});

	test('keeps empty states visible and offers a way out when refresh cannot populate data', () => {
		expect(source).toContain('No market snapshot yet');
		expect(source).toContain('No provider leaders for');
		expect(source).toContain('Global data is available above');
		expect(source).toContain('NOOR tried to refresh the provider matrix');
		expect(source).toContain('Refresh charts');
		expect(source).toContain('<ErrorState');
		expect(source).not.toContain('Restart the NOOR server');
	});

	test('shows the full provider matrix target, not just Spotify', () => {
		expect(source).toContain('matrix.providers as provider');
		expect(source).toContain('{provider.label}');
		expect(source).toContain('row.cells[provider.source_key]');
		expect(source).toContain('matrixHasData(');
		expect(source).toContain('regionHasMatrixData($selectedChartRegion)');
		// Borderless table: a hairline under the header, fills for hover and selection.
		expect(source).toContain('border-bottom: 1px solid var(--border-subtle)');
		expect(source).not.toContain('Tap to resolve');
		expect(source).not.toContain('Provider comparison');
	});

	test('uses shared segmented controls and a top 20 mural instead of duplicate leader cards', () => {
		expect(source).toContain('SectionHeader');
		expect(source).toContain('variant="charts"');
		expect(source).toContain('level={2}');
		expect(source).toContain('<Segmented');
		expect(source).toContain('label="Daily chart region"');
		expect(source).toContain('label="Daily chart provider"');
		expect(source).toContain('ChartMural');
		expect(source).toContain('type ChartMuralItem');
		expect(muralSource).toContain('chart-mural');
		expect(source).toContain('chartEntries.map((entry');
		expect(source).toContain('currentEntry');
		expect(source).not.toContain('provider-card');
		expect(source).not.toContain('chart-mural-card');
	});

	test('resolves chart entries against TIDAL through the shared, cached matcher', () => {
		// Through the shared gate, not a bare api.searchTidal: a page resolves its
		// whole visible list at once.
		expect(matchSource).toContain("from '$lib/actions/lazy-tidal-art'");
		expect(matchSource).toContain('gatedTidalSearch(');
		// A paused breaker is an outage, not a miss: it must not be cached.
		expect(matchSource).toContain("if (!results) throw new Error('TIDAL search paused');");
		expect(matchSource).toContain('const playable = tidalSearchTrackToPlayable(hit);');
		expect(matchSource).toContain('artwork_url: playable.artwork_url ?? fallbackArtwork');
		expect(source).toContain('chartMatchKey(item.artist, item.title)');
		expect(source).toContain('peekChartMatch(key)');
		expect(source).toContain('playTidalTrackNow');
		expect(source).toContain('TIDAL ready');
		expect(muralSource).toContain('chart-mural-art');
		expect(source).toContain('async function playItem(item');
	});

	test('plays entries the server already resolved without searching again', () => {
		expect(source).toContain('if (item.tidal_id) return directPlayable(item);');
		expect(source).toContain('await playTrackNow(item.local_track_id);');
	});

	test('uses shared context menus', () => {
		expect(source).toContain('openContextMenu');
		expect(source).toContain('buildTidalTrackMenu');
		expect(source).toContain('openItemContext');
		expect(source).toContain('oncontextmenu');
	});

	test('drops late chart work after the route is destroyed', () => {
		expect(source).toContain('let destroyed = false;');
		expect(source).toContain('onDestroy(() => {');
		expect(source).toContain('destroyed = true;');
		expect(source).toContain('return query.subscribe((state) => untrack(() => {');
		expect(source).toContain('if (!destroyed) matches[key] = hit;');
	});

	test('keeps chart page titles neutral with shared headers and artwork images', () => {
		expect(chartsPage).toContain('<PageHeader');
		expect(chartsPage).not.toContain('variant="editorial"');
		expect(chartsPage).toContain('<SectionHeader');
		expect(chartsPage).toContain('<ArtworkImage');
		expect(chartsPage).toContain('className="chart-playlist-art"');
		expect(chartsPage).not.toContain('service-spotify');
		expect(source).not.toContain('service-spotify');
		expect(muralSource).toContain('.chart-mural-title');
		expect(muralSource).toContain('color: var(--text-primary)');
	});

	test('keeps chart playlist card menus app-owned', () => {
		expect(chartsPage).toContain('function openChartPlaylistContext(e: MouseEvent');
		expect(chartsPage).toContain('e.preventDefault();');
		expect(chartsPage).toContain('e.stopPropagation();');
		expect(chartsPage).toContain('openContextMenu(e, chartMenu(chart.id, chart.title), chart.title);');
		expect(chartsPage).toContain('oncontextmenu={(e) => openChartPlaylistContext(e, c)}');
	});

	test('loads playlist card covers through the metadata endpoint', () => {
		expect(chartsPage).toContain('api.getSpotifyPlaylistMeta(c.id, signal)');
		expect(chartsPage).not.toContain('api.getSpotifyPlaylist(c.id');
	});
});
