import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'TrendingShelf.svelte'), 'utf8');
const muralSource = readFileSync(join(here, 'ChartMural.svelte'), 'utf8');

describe('trending shelf contract', () => {
	test('renders Last.fm charts with the market-pulse mural treatment', () => {
		expect(source).toContain('ChartMural');
		expect(source).toContain('type ChartMuralItem');
		expect(source).toContain('<ChartMural');
		expect(source).toContain('accent="lastfm"');
		expect(source).toContain('muralItems');
		expect(muralSource).toContain('chart-mural');
		expect(muralSource).toContain('chart-mural-bg');
		expect(muralSource).toContain('chart-mural-tile');
		expect(muralSource).toContain('chart-mural-art');
		expect(source).toContain('currentEntry');
		expect(source).toContain('visibleEntries');
		expect(source).toContain('ROTATE_MS');
		expect(muralSource).toContain('repeat(10, minmax(0, 1fr))');
		expect(muralSource).toContain('repeat(5, minmax(0, 1fr))');
		expect(muralSource).toContain('ArtworkImage');
		expect(muralSource).toContain('size={320}');
		expect(muralSource).toContain('use:lazyTidalArt');
		expect(source).not.toContain('TrendingCard');
		expect(source).not.toContain('lastfm-mural');
	});

	test('keeps Last.fm scopes, cache, and empty states wired', () => {
		expect(source).toContain('SectionHeader');
		expect(source).toContain('variant="charts"');
		expect(source).toContain('level={2}');
		expect(source).toContain('cachedApi.lastfmCountriesQuery()');
		expect(source).toContain('cachedApi.lastfmGenresQuery()');
		expect(source).toContain("return { source: 'lastfm', limit, country };");
		expect(source).toContain("return { source: 'lastfm', limit, tag: genre };");
		// Persisted, revalidating per-scope query instead of a session-only map.
		expect(source).toContain('cachedApi.trendingQuery(trendingQuery(mode, country, genre))');
		expect(source).not.toContain('trending-cache');
		expect(source).toContain('Couldn');
		expect(source).toContain('<ErrorState');
		expect(source).toContain('onretry={retry}');
		expect(source).toContain('Nothing trending here yet');
		expect(source).not.toContain('lastfm-more');
		expect(source).not.toContain('lastfm-chart-list');
		expect(source).not.toContain('artistSignals');
		expect(source).not.toContain('tagSignals');
		expect(source).not.toContain('loadSignalPanels');
		expect(source).not.toContain('lastfm-signal-grid');
		expect(source).not.toContain('artist-signal-art');
		expect(source).not.toContain('tag-signal-chip');
	});

	test('ignores stale trending loads and cached-scope races', () => {
		// The scope effect owns its subscription, so leaving a scope tears it down
		// and a late answer for that scope can never land on the next one.
		expect(source).toContain("import { onMount, untrack } from 'svelte';");
		expect(source).toContain('return query.subscribe((state) => untrack(() => {');
		expect(source).toContain('tracksToken = token;');
		// The previous scope stays on screen, dimmed, while the next one loads.
		expect(source).toContain('let showingOtherScope = $derived(tracks.length > 0 && tracksToken !== activeToken);');
		expect(source).toContain('stale={showingOtherScope && loading}');
		expect(source).toContain("if (mode === 'country' && !countriesReady) return;");
	});

	test('preserves playback, TIDAL resolution, artwork fallback, and menus', () => {
		expect(source).toContain('playTrackNow');
		expect(source).toContain('playChartTidalTrack');
		expect(source).toContain('isEntryUnresolved');
		expect(source).toContain('Matched on TIDAL when played');
		// The Last.fm placeholder guard used to be a private copy in this file.
		// It now lives in $lib/utils/artwork so the home recommendation shelves
		// get it too; what matters here is that this shelf still routes its
		// artwork through it. The hash itself is covered by that module's tests.
		expect(source).toContain("from '$lib/utils/artwork'");
		expect(source).toContain('usableArtwork(');
		expect(source).toContain('needsLazyArtwork');
		expect(source).toContain('buildTrackMenu');
		expect(source).toContain('buildTidalTrackMenu');
		expect(source).toContain('handleEntryContext');
		expect(source).toContain('onCardContext');
		expect(source).toContain('onItemContext');
	});

	test('uses the shared Segmented and FilterChip controls', () => {
		expect(source).toContain('<Segmented');
		expect(source).toContain('label="Trending scope"');
		expect(source).toContain('<FilterChip pressed={c.code === $selectedCountry}');
		expect(source).toContain('<FilterChip pressed={g.key === $selectedGenre}');
		expect(source).not.toContain('class="chip"');
		expect(source).not.toContain('class="chip secondary"');
		expect(source).not.toContain('padding: 4px 10px');
	});

	test('keeps mural titles neutral and does not repeat the section source', () => {
		expect(muralSource).toContain('.chart-mural-title');
		expect(muralSource).toContain('color: var(--text-primary)');
		// The section header names the chart; the mural does not repeat it.
		expect(muralSource).not.toContain('chart-mural-kind');
		expect(muralSource).not.toContain('.chart-mural-title {\n\t\tcolor: var(--chart-mural-accent)');
	});

	test('keeps mural navigation controls out of the title area', () => {
		// One pager in the top-right corner, away from the title on the left.
		expect(muralSource).toContain('<div class="chart-pager">');
		expect(muralSource).toContain('right: var(--space-4)');
		expect(muralSource).not.toContain('top: 50%');
		expect(muralSource).not.toContain('chart-mural-loading');
		expect(muralSource).not.toContain('left: var(--space-3)');
	});

	test('fits short mural item sets instead of forcing every panel into twenty slots', () => {
		expect(muralSource).toContain('muralLayoutClass(items.length)');
		expect(muralSource).toContain("if (count <= 12) return 'layout-count-12'");
		expect(muralSource).toContain('.chart-mural-bg.layout-count-6');
		expect(muralSource).toContain('grid-template-columns: repeat(3, minmax(0, 1fr))');
		expect(muralSource).toContain('.chart-mural-bg.layout-count-12');
		expect(muralSource).toContain('grid-template-columns: repeat(6, minmax(0, 1fr))');
		expect(muralSource).toContain('.chart-mural-bg.layout-count-16');
		expect(muralSource).toContain('grid-template-columns: repeat(8, minmax(0, 1fr))');
	});
});
