import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, 'RemoteQueue.svelte'), 'utf8');

describe('remote queue playback contract', () => {
	test('remote queue can play TIDAL queue rows as well as local rows', () => {
		expect(source).toContain('playQueueItemNow,');
		expect(source).toContain("import { queueItemToTidalPlayable } from '$lib/utils/track';");
		expect(source).toContain('item.track.id > 0 || queueItemToTidalPlayable(item) != null');
		expect(source).toContain('await playQueueItemNow(item.id);');
		expect(source).not.toContain('item.id < 0');
		expect(source).toContain('items: buildTidalTrackMenu(tidal, { inQueue: true, remoteRoutes: true })');
		expect(source).not.toContain('playTidalTrackNow');
		expect(source).not.toContain('playTrackNow(item.track.id)');
		expect(source).toContain('disabled={!canPlay(item) || isCurrent(item)}');
		expect(source).not.toContain('t.id <= 0 && !!t.tidal_id');
	});
});
