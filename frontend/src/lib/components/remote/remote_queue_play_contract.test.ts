import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, 'RemoteQueue.svelte'), 'utf8');

describe('remote queue playback contract', () => {
	test('remote queue can play TIDAL queue rows as well as local rows', () => {
		expect(source).toContain('playTidalTrackNow,');
		expect(source).toContain("import { queueItemToTidalPlayable } from '$lib/utils/track';");
		expect(source).toContain('item.track.id > 0 || queueItemToTidalPlayable(item) != null');
		expect(source).toContain('const tidal = queueItemToTidalPlayable(item);');
		expect(source).toContain('tidal != null && item.id < 0');
		expect(source).toContain('items: buildTidalTrackMenu(tidal, { inQueue: true, remoteRoutes: true })');
		expect(source).toContain('await playTidalTrackNow(tidal);');
		expect(source).toContain('await playTrackNow(item.track.id);');
		expect(source).toContain('disabled={!canPlay(item) || isCurrent(item)}');
		expect(source).not.toContain('t.id <= 0 && !!t.tidal_id');
	});
});
