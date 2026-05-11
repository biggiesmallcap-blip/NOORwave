import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(import.meta.dirname, '../src/routes/videos/+page.svelte'), 'utf8');

describe('Videos empty player prompt', () => {
	test('choose-video prompt only appears after a search has selectable results', () => {
		expect(source).toContain('let hasVideoChoices = $derived');
		expect(source).toContain('let showChooseVideoPrompt = $derived');
		expect(source).toContain('let showVideoHero = $derived');
		expect(source).toContain('query.trim().length > 0');
		expect(source).toContain('hasVideoChoices');
		expect(source).toContain('{:else if showChooseVideoPrompt}');
	});

	test('plain empty Videos state hides the whole hero panel', () => {
		expect(source).toContain('{#if showVideoHero}');
		expect(source).toContain('class:hero--prompt={showChooseVideoPrompt}');
		expect(source).not.toContain('class="video-player-quiet"');
		expect(source).not.toContain('<EmptyState title="Choose a video"');
		expect(source).not.toContain('A focused TIDAL video surface with audio queue state preserved.');
	});

	test('choose-video prompt has a scoped entrance animation', () => {
		expect(source).toContain('.video-choice-prompt');
		expect(source).toContain('animation: video-choice-prompt-in');
		expect(source).toContain('@keyframes video-choice-prompt-in');
	});

	test('landing chips sit in the same centered column as the search input', () => {
		const landingBlock = source.match(/\.landing-row\s*\{([\s\S]*?)\n\s*\}/)?.[1] ?? '';
		const railHeaderBlock = source.match(/\.rail-header,\s*\n\s*\.section-heading\s*\{([\s\S]*?)\n\s*\}/)?.[1] ?? '';
		const chipsBlock = source.match(/\.chips\s*\{([\s\S]*?)\n\s*\}/)?.[1] ?? '';

		expect(landingBlock).toContain('width: 100%');
		expect(landingBlock).toContain('max-width: 720px');
		expect(landingBlock).toContain('margin: 0 auto');
		expect(railHeaderBlock).toContain('align-items: baseline');
		expect(chipsBlock).toContain('justify-content: flex-start');
	});
});
