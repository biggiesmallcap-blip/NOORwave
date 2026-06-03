import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, 'RemoteQueue.svelte'), 'utf8');

describe('remote queue artwork safety', () => {
	test('queue artwork uses fixed TIDAL sizing with error fallback', () => {
		expect(source).toContain('let failedArtworkUrls = $state<Record<string, boolean>>({});');
		expect(source).toContain('function queueArtwork(rawUrl: string | null | undefined): string | null');
		expect(source).toContain('upscaleTidalArtwork(rawUrl, 320)');
		expect(source).toContain('function markArtworkFailed(renderedUrl: string | null)');
		expect(source).toContain('{@const queueArt = queueArtwork(item.track.artwork_url)}');
		expect(source).toContain('{#if queueArt}');
		expect(source).toContain('src={queueArt}');
		expect(source).toContain('onerror={() => markArtworkFailed(queueArt)}');
		expect(source).toContain('remote-queue-thumb-empty');
		expect(source).not.toContain('src={upscaleTidalArtwork(item.track.artwork_url, 320)}');
	});
});
