import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, 'RemoteTrackRow.svelte'), 'utf8');

describe('remote track row contract', () => {
	test('TIDAL rows preserve local and favorite metadata for menus and playback', () => {
		expect(source).toContain('await playTidalTrackNow(tidalShape(track as TidalPlayable | TidalDiscographyTrack));');
		expect(source).toContain('items: buildTidalTrackMenu(t, { remoteRoutes: true })');
		expect(source).toContain("track_id: 'track_id' in t ? t.track_id : undefined");
		expect(source).toContain("local_id: 'local_id' in t ? t.local_id : null");
		expect(source).toContain("is_in_library: 'is_in_library' in t ? t.is_in_library : undefined");
		expect(source).toContain("is_favorite: 'is_favorite' in t ? t.is_favorite : undefined");
		expect(source).toContain('items: buildTrackMenu(asMenuTrack(t), { remoteRoutes: true })');
	});
});
