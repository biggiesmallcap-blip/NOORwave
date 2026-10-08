// What the Library Songs tab lists. 'liked' (default) matches TIDAL and
// Spotify; 'library' also shows songs saved through albums or local imports.
import { createPersistedStore, oneOf } from './persisted';

export type LibrarySongsScope = 'liked' | 'library';

export const librarySongsScope = createPersistedStore<LibrarySongsScope>('noor.library.songsScope', 'liked', {
	parse: oneOf(['liked', 'library'] as const),
});
