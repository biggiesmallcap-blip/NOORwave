import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const srcRoot = resolve(here, '../../..');

function source(path: string): string {
	return readFileSync(join(srcRoot, path), 'utf8');
}

const remotePage = source('routes/remote/+page.svelte');
const remoteQueue = source('lib/components/remote/RemoteQueue.svelte');

describe('remote queue current row contract', () => {
	test('remote queue receives and validates current queue item id', () => {
		expect(remotePage).toContain('currentQueueItemId,');
		expect(remotePage).toContain('currentQueueItemId={$currentQueueItemId}');
		expect(remoteQueue).toContain("import { isQueueItemActive } from '$lib/player/queue_active';");
		expect(remoteQueue).toContain('currentQueueItemId = null');
		expect(remoteQueue).toContain('currentQueueItemId?: number | null');
		expect(remoteQueue).toContain('return isQueueItemActive(item, current, currentQueueItemId, displayQueue);');
		expect(remoteQueue).not.toContain('return current?.id != null && item.track.id === current.id;');
	});
});
