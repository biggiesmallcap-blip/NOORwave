import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(import.meta.dirname, '../src/routes/videos/+page.svelte'), 'utf8');

describe('Videos editorial browse state', () => {
	test('hero slot arbitration: the layer yields to an active video session', () => {
		// The editorial layer only renders when no video session owns the page,
		// and it fades rather than cuts.
		expect(source).toContain('let videoSessionActive = $derived');
		expect(source).toContain('let showEditorialLayer = $derived(!videoSessionActive');
		expect(source).toContain('{#if showEditorialLayer}');
		expect(source).toContain('transition:fade');
	});

	test('search focus recedes the layer without unmounting it', () => {
		// class-driven fade/collapse: the mural stays mounted so clearing the
		// field restores the same set at the same tile index.
		expect(source).toContain('let searchFocused = $derived(query.trim().length > 0)');
		expect(source).toContain('class:receded={searchFocused}');
		expect(source).toContain('.editorial-layer.receded');
		expect(source).toContain('grid-template-rows: 0fr');
		expect(source).toContain('pointer-events: none');
	});

	test('daily picks feed the mural and play through the shared video queue', () => {
		expect(source).toContain("discoverSets.find((s) => s.slug === 'daily-picks')");
		expect(source).toContain('<ChartMural');
		expect(source).toContain('queue: set.items');
		// Set plays route through the store controller, never an inline player.
		expect(source).toContain('await playVideo(video, {');
		expect(source).not.toContain('<VideoPlayer');
	});

	test('mural rotation pauses under focus, playback, and hover', () => {
		expect(source).toContain('MURAL_ROTATE_MS');
		expect(source).toContain('if (!muralPaused && !searchFocused && !videoSessionActive) jumpMural(1);');
		expect(source).toContain('onPauseChange={(paused) => (muralPaused = paused)}');
	});

	test('TIDAL editorial folds in as a shelf with an outbound link', () => {
		expect(source).toContain("api.getTidalPage('videos')");
		expect(source).toContain("From TIDAL's desk");
		expect(source).toContain('href="/tidal/videos">More from TIDAL</a>');
	});

	test('legacy landing chips only appear when there is no editorial content', () => {
		expect(source).toContain('!hasBrowseContent && !loadingBrowse}');
	});
});
