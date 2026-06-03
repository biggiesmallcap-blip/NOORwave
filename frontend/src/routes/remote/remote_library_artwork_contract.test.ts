import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(
	resolve(__dirname, 'library', '+page.svelte'),
	'utf8'
);

describe('remote library artwork safety', () => {
	test('remote library artwork uses fixed TIDAL sizes with error fallbacks', () => {
		expect(source).toContain('let failedArtworkUrls = $state<Record<string, boolean>>({});');
		expect(source).toContain('function artworkCandidate(');
		expect(source).toContain('upscaleTidalArtwork(rawUrl, size)');
		expect(source).toContain('function markArtworkFailed(renderedUrl: string | null)');
		expect(source).toContain('onerror={() => markArtworkFailed(recentArt)}');
		expect(source).toContain('onerror={() => markArtworkFailed(artistArt)}');
		expect(source).toContain('onerror={() => markArtworkFailed(albumArt)}');
		expect(source).toContain('onerror={() => markArtworkFailed(trackArt)}');
		expect(source).not.toContain('src={upscaleTidalArtwork(entry.artwork_url, 320)}');
		expect(source).not.toContain('src={upscaleTidalArtwork(album.artwork_url, 320)}');
		expect(source).not.toContain('src={upscaleTidalArtwork(track.artwork_url, 320)}');
	});
});
