import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'RankList.svelte'), 'utf8');

describe('RankList route contract', () => {
	test('keeps analytics row context menus app-owned', () => {
		expect(source).toContain('function onTrackContext(event: MouseEvent, t: AnalyticsTopTrack)');
		expect(source).toContain('function onArtistContext(event: MouseEvent, a: AnalyticsTopArtist)');
		expect(source).toContain('event.preventDefault();');
		expect(source).toContain('event.stopPropagation();');
		expect(source).toContain('openContextMenu(event, buildTrackMenu(trackMenuTrack(t)))');
		expect(source).toContain('buildArtistMenu({ id: a.artist_id, name: a.artist_name, in_library: true })');
		expect(source).toContain('onclick={() => activateTrack(t)}');
		expect(source).toContain('onclick={() => activateArtist(a)}');
	});
});
