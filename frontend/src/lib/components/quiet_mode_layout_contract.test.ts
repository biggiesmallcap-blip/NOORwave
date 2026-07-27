import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'QuietMode.svelte'), 'utf8');

function cssBlock(selector: string): string {
	const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
	const match = source.match(new RegExp(`${escaped}\\s*\\{(?<body>[^}]*)\\}`));
	if (!match?.groups?.body) {
		throw new Error(`Missing CSS block for ${selector}`);
	}
	return match.groups.body;
}

describe('quiet mode layout contracts', () => {
	test('routes current-track artwork through TIDAL fallback sizes', () => {
		expect(source).toContain('tidalArtworkFallbackSizes');
		expect(source).toContain(
			'let quietArtworkBase = $derived(\n\t\tartworkCandidate($currentTrack?.artwork_url, QUIET_ART_BASE_SIZE)\n\t);'
		);
		expect(source).toContain('const QUIET_ART_BASE_SIZE = 640;');
		expect(source).toContain('onerror={() => markArtworkFailed(quietArtwork)}');
		expect(source).not.toContain('src={$currentTrack.artwork_url}');
	});

	test('opens on the artwork the player bar already decoded', () => {
		// A cold 1280 fetch on open made the cover arrive in progressive-JPEG stages.
		expect(source).not.toContain('artworkCandidate($currentTrack?.artwork_url, 1280)');
		expect(source).toContain('src={quietArtworkBase}');

		// Sharper copy is decoded off-screen first, so the swap is one atomic frame.
		expect(source).toContain('await preload.decode();');
		expect(source).toContain('upgradedArt = { source, url, size: target }');
		expect(source).toContain('if (target <= QUIET_ART_BASE_SIZE) return;');
	});

	test('reveals artwork on decode rather than on mount', () => {
		expect(source).not.toContain('animation: quiet-art-fade');
		expect(source).not.toContain('@keyframes quiet-art-fade');
		expect(source).toContain('onload={(e) => markArtworkReady(e.currentTarget as HTMLImageElement)}');
		expect(source).toContain('await img.decode();');
		expect(source).toContain('class:is-ready={artReady}');

		const art = cssBlock('.quiet-art-img');
		expect(art).toContain('opacity: 0');
		expect(art).toContain('transition: opacity');

		const backdropArt = cssBlock('.quiet-backdrop-art');
		expect(backdropArt).toContain('opacity: 0');
		expect(cssBlock('.quiet-backdrop-art.is-ready')).toContain('opacity: 0.35');
	});

	test('artwork scales fluidly so 1080p is not locked to the max cover size', () => {
		expect(source).toContain('--quiet-art-size: clamp(');
		expect(source).toContain('--quiet-panel-w: min(var(--quiet-art-size), calc(100vw - (2 * var(--quiet-panel-pad))));');

		const panel = cssBlock('.quiet-panel');
		expect(panel).toContain('grid-template-columns: minmax(0, var(--quiet-panel-w))');
		expect(panel).toContain('gap: var(--quiet-panel-gap)');
		expect(panel).toContain('padding: var(--quiet-panel-pad)');

		const art = cssBlock('.quiet-art-wrap');
		expect(art).toContain('width: var(--quiet-panel-w)');
		expect(art).not.toContain('max-width: min(60vh, 520px)');
	});
});
