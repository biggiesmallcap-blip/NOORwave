import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, 'RemoteTransport.svelte'), 'utf8');

describe('remote transport favorite contract', () => {
	test('TIDAL ephemeral tracks can use the existing favorite import path', () => {
		expect(source).toContain('let canFavorite = $derived(!!track && (track.id > 0 || !!track.tidal_id));');
		expect(source).toContain('if (!track || (track.id <= 0 && !track.tidal_id)) return;');
		expect(source).toContain('void toggleTrackFavorite(track.id, track.is_favorite ?? false);');
		expect(source).toContain('disabled={!canFavorite}');
		expect(source).not.toContain('disabled={!track || track.id <= 0}');
	});
});
