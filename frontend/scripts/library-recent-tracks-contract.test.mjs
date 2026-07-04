import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

describe('library recent tracks contract', () => {
	test('recent tracks are sourced from play history, not the favorited library', () => {
		const source = readFileSync('src/routes/library/+page.svelte', 'utf8');

		expect(source).toContain("const RECENT_TRACK_LIMIT = 10;");
		expect(source).toContain('recentTracks: [] as CachedTrack[],');
		expect(source).toContain('let recentTracks = $state<Track[]>(homePanelCandidateCache.recentTracks);');
		expect(source).toContain('async function loadRecentTracks()');
		// Sourced from listen_history so externally-played tracks (radio, discover)
		// show up too - not the favorite_only library query.
		expect(source).toContain('cachedApi.getHistory(RECENT_TRACK_LIMIT, 0)');
		expect(source).not.toContain(
			"cachedApi.getTracks('last_played_at', 'desc', RECENT_TRACK_LIMIT, 0, true, false)",
		);
		expect(source).toContain('homePanelCandidateCache.recentTracks = recentTracks;');
		expect(source).not.toContain('let recentTracks = $derived.by(() =>');
	});

	test('view all opens the full history route', () => {
		const source = readFileSync('src/routes/library/+page.svelte', 'utf8');

		expect(source).toContain("void goto('/history')");
	});

	test('recent tracks refresh when listen history changes', () => {
		const source = readFileSync('src/routes/library/+page.svelte', 'utf8');

		expect(source).toContain("import { wsMessages } from '$lib/api/ws';");
		expect(source).toContain('wsMessages.subscribe');
		expect(source).toContain("latest.type === 'listen_history_updated'");
		expect(source).toContain('void loadRecentTracks();');
	});
});
