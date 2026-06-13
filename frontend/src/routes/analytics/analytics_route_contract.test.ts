import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('./+page.svelte', import.meta.url), 'utf8');

describe('/analytics route load contract', () => {
	test('ignores stale analytics loads across range, refresh, and websocket updates', () => {
		expect(source).toContain('let loadSeq = 0;');
		expect(source).toContain("type AnalyticsLoadReason = 'initial' | 'window' | 'refresh';");
		expect(source).toContain('const seq = ++loadSeq;');
		expect(source).toContain('void loadSignals(\'window\', d);');
		// Routed through the cache layer for instant-paint + stale-while-revalidate,
		// with the initial range seeded synchronously from the persisted snapshot.
		expect(source).toContain('const nextSignals = await cachedApi.getAnalyticsSignals(requestedDays);');
		expect(source).toContain('cachedApi.analyticsSignalsQuery(untrack(() => days)).getSnapshot().data');
		expect(source).toContain('if (seq !== loadSeq) return;');
		expect(source).toContain("const debouncedWsRefresh = debounce(() => void loadSignals('refresh'), 1500);");
		expect(source).toContain('loadSeq += 1;');
	});
});
