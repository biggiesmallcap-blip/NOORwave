import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

describe('library recent tracks contract', () => {
	test('recent tracks use a dedicated last-played query', () => {
		const source = readFileSync('src/routes/library/+page.svelte', 'utf8');

		expect(source).toContain("const RECENT_TRACK_LIMIT = 10;");
		expect(source).toContain('let recentTracks = $state<Track[]>([]);');
		expect(source).toContain('async function loadRecentTracks()');
		expect(source).toContain(
			"api.getTracks('last_played_at', 'desc', RECENT_TRACK_LIMIT, 0, true, false)",
		);
		expect(source).toContain('filter((track) => track.last_played_at)');
		expect(source).not.toContain('let recentTracks = $derived.by(() =>');
	});

	test('recent tracks refresh when listen history changes', () => {
		const source = readFileSync('src/routes/library/+page.svelte', 'utf8');

		expect(source).toContain("import { wsMessages } from '$lib/api/ws';");
		expect(source).toContain('wsMessages.subscribe');
		expect(source).toContain("latest.type === 'listen_history_updated'");
		expect(source).toContain('void loadRecentTracks();');
	});
});
