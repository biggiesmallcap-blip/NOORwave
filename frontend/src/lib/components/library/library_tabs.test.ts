import { describe, expect, test } from 'vitest';
import { restoreLibraryTab, LIBRARY_TABS } from './library_tabs';

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
