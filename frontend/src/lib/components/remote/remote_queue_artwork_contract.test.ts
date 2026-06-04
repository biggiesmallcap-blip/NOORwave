import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, 'RemoteQueue.svelte'), 'utf8');

describe('remote queue artwork safety', () => {
	test('queue artwork uses fixed TIDAL sizing with error fallback', () => {
		expect(source).toContain('let failedArtworkUrls = $state<Record<string, boolean>>({});');
		expect(source).toContain('tidalArtworkFallbackSizes');
		expect(source).toContain('function queueArtwork(item: QueueItem, size: TidalArtworkSize = 320): string | null');
		expect(source).toContain('if (item.is_pending) return null;');
		expect(source).toContain('for (const fallbackSize of tidalArtworkFallbackSizes(rawUrl, size))');
		expect(source).toContain('upscaleTidalArtwork(rawUrl, fallbackSize)');
		expect(source).toContain('function markArtworkFailed(renderedUrl: string | null)');
		expect(source).toContain('{@const queueArt = queueArtwork(item)}');
		expect(source).toContain('{#if queueArt}');
		expect(source).toContain('src={queueArt}');
		expect(source).toContain('onerror={() => markArtworkFailed(queueArt)}');
		expect(source).toContain('remote-queue-thumb-empty');
		expect(source).not.toContain('src={upscaleTidalArtwork(item.track.artwork_url, 320)}');
	});
});
