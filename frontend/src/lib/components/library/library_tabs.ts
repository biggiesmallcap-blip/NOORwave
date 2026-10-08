// Library views. Songs is your liked songs, as in TIDAL and Spotify, where the
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
