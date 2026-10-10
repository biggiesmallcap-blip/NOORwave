import { describe, expect, test } from 'vitest';
import { cachedTabCounts, rememberTabCounts, restoreLibraryTab, LIBRARY_TABS, tabCountLabel, viewCountLabel } from './library_tabs';

describe('library tabs', () => {
	test('Liked is no longer a tab; Tracks is called Songs', () => {
		expect(LIBRARY_TABS.map((tab) => tab.id)).toEqual(['all', 'tracks', 'albums', 'artists']);
		expect(LIBRARY_TABS.find((tab) => tab.id === 'tracks')?.label).toBe('Songs');
	});

	test('saved sessions on the old Liked tab land on Songs', () => {
		expect(restoreLibraryTab('liked')).toBe('tracks');
		expect(restoreLibraryTab('albums')).toBe('albums');
		expect(restoreLibraryTab('nonsense')).toBe('all');
	});
});

describe('counts', () => {
	test('tab counts are compact; unknown counts are hidden', () => {
		expect(tabCountLabel(4538)).toBe('4.5k');
		expect(tabCountLabel(812)).toBe('812');
		expect(tabCountLabel(null)).toBeNull();
	});

	test('the toolbar count is exact and names the view', () => {
		expect(viewCountLabel('tracks', 4179, true)).toBe('4,179 songs');
		expect(viewCountLabel('tracks', 4538, false)).toBe('4,538 library songs');
		expect(viewCountLabel('albums', 1, false)).toBe('1 album');
		expect(viewCountLabel('all', 10, false)).toBeNull();
	});

	test('tab counts are remembered per Songs scope so the pills paint at full width', () => {
		expect(cachedTabCounts(true)).toEqual({ tracks: null, albums: null });
		rememberTabCounts(true, { tracks: 4179, albums: 2400 });
		rememberTabCounts(false, { tracks: 4538, albums: 2400 });
		expect(cachedTabCounts(true)).toEqual({ tracks: 4179, albums: 2400 });
		expect(cachedTabCounts(false)).toEqual({ tracks: 4538, albums: 2400 });
	});
});
