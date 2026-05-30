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
		expect(source).toContain('api.getLastfmCountries()');
		expect(source).toContain('api.getLastfmGenres()');
		expect(source).toContain("api.getTrending({ source: 'lastfm', limit, country })");
		expect(source).toContain("api.getTrending({ source: 'lastfm', limit, tag: genre })");
		expect(source).toContain('getCached(token)');
		expect(source).toContain('putCached(token, next)');
		expect(source).toContain('Couldn');
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

	test('preserves playback, TIDAL resolution, artwork fallback, and menus', () => {
		expect(source).toContain('playTrackNow');
		expect(source).toContain('playChartTidalTrack');
		expect(source).toContain('isEntryUnresolved');
		expect(source).toContain('Resolve on TIDAL');
		expect(source).toContain('LASTFM_PLACEHOLDER_HASH');
		expect(source).toContain('needsLazyArtwork');
		expect(source).toContain('buildTrackMenu');
		expect(source).toContain('buildTidalTrackMenu');
		expect(source).toContain('handleEntryContext');
		expect(source).toContain('onCardContext');
		expect(source).toContain('onItemContext');
	});

	test('standardizes pill controls with bordered active chips', () => {
		expect(source).toContain('border: 1px solid var(--panel-border)');
		expect(source).toContain('border-color: var(--accent-line)');
		expect(source).toContain('padding: var(--space-2) var(--space-3)');
		expect(source).not.toContain('padding: 4px 10px');
	});

	test('keeps mural titles neutral while allowing small source accents', () => {
		expect(muralSource).toContain('.chart-mural-title');
		expect(muralSource).toContain('color: var(--text-primary)');
		expect(muralSource).toContain('.chart-mural-kind');
		expect(muralSource).toContain('color: var(--chart-mural-accent)');
		expect(muralSource).not.toContain('.chart-mural-title {\n\t\tcolor: var(--chart-mural-accent)');
	});
});
