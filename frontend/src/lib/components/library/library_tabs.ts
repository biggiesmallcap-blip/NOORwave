// Library views. Songs is your liked songs, where the
// heart is the save. The old Liked tab listed ~92% of the same songs as
// Tracks, so it is gone; Settings > Library can widen Songs to every library
// song (librarySongsScope). Saved sessions from before keep working.
export type LibraryTab = 'all' | 'tracks' | 'albums' | 'artists';

export const LIBRARY_TABS: readonly { id: LibraryTab; label: string }[] = [
	{ id: 'all', label: 'All' },
	{ id: 'tracks', label: 'Songs' },
	{ id: 'albums', label: 'Albums' },
	{ id: 'artists', label: 'Artists' },
];

export function restoreLibraryTab(saved: unknown): LibraryTab {
	if (saved === 'liked') return 'tracks';
	return LIBRARY_TABS.some((t) => t.id === saved) ? (saved as LibraryTab) : 'all';
}

const compact = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 });
const exact = new Intl.NumberFormat('en');

/** Short count beside a tab label; null hides it (e.g. artists, which have no total). */
export function tabCountLabel(count: number | null | undefined): string | null {
	if (count == null) return null;
	return count < 1000 ? String(count) : compact.format(count).toLowerCase();
}

/** The exact count at the start of the toolbar; null for views without one. */
export function viewCountLabel(tab: LibraryTab, count: number, likedOnly: boolean): string | null {
	if (tab === 'all' || tab === 'artists') return null;
	const noun = tab === 'albums' ? 'album' : likedOnly ? 'song' : 'library song';
	return `${exact.format(count)} ${noun}${count === 1 ? '' : 's'}`;
}

export type LibraryTabCounts = { tracks: number | null; albums: number | null };

// Last known tab counts per Songs scope, so the pill row paints at its final
// width instead of reflowing when the one-row count queries land. Kept in
// memory across visits and in localStorage across launches; every storage
// access is guarded (a full quota must never break the page).
const TAB_COUNTS_STORAGE_KEY = 'library.tabCounts.v1';
const tabCountsMemory: Partial<Record<'liked' | 'all', LibraryTabCounts>> = {};

function scopeKey(likedOnly: boolean): 'liked' | 'all' {
	return likedOnly ? 'liked' : 'all';
}

function readStoredTabCounts(): Partial<Record<'liked' | 'all', LibraryTabCounts>> {
	try {
		const raw = globalThis.localStorage?.getItem(TAB_COUNTS_STORAGE_KEY);
		const parsed = raw ? JSON.parse(raw) : null;
		return parsed && typeof parsed === 'object' ? parsed : {};
	} catch {
		return {};
	}
}

export function cachedTabCounts(likedOnly: boolean): LibraryTabCounts {
	const key = scopeKey(likedOnly);
	const hit = tabCountsMemory[key] ?? readStoredTabCounts()[key];
	if (hit && (typeof hit.tracks === 'number' || hit.tracks === null) && (typeof hit.albums === 'number' || hit.albums === null)) {
		tabCountsMemory[key] = hit;
		return hit;
	}
	return { tracks: null, albums: null };
}

export function rememberTabCounts(likedOnly: boolean, counts: LibraryTabCounts): void {
	const key = scopeKey(likedOnly);
	tabCountsMemory[key] = counts;
	try {
		globalThis.localStorage?.setItem(TAB_COUNTS_STORAGE_KEY, JSON.stringify({ ...readStoredTabCounts(), [key]: counts }));
	} catch {
		// Storage full or blocked: the in-memory copy still covers this session.
	}
}
