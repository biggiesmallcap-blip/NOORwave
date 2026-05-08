import { writable, get } from 'svelte/store';
import { api, type Playlist, type Genre } from '$lib/api/client';

const TTL_MS = 60_000;

export const catalogPlaylists = writable<Playlist[]>([]);
export const catalogGenres = writable<Genre[]>([]);

let playlistsFetchedAt = 0;
let genresFetchedAt = 0;
let playlistsInflight: Promise<Playlist[]> | null = null;
let genresInflight: Promise<Genre[]> | null = null;
let playlistsMutationEpoch = 0;
let genresMutationEpoch = 0;

export async function ensureCatalogPlaylists(): Promise<Playlist[]> {
    const now = Date.now();
    if (now - playlistsFetchedAt < TTL_MS && get(catalogPlaylists).length > 0) {
        return get(catalogPlaylists);
    }
    // Note: invalidate() during in-flight does NOT cancel; the existing fetch
    // resolves and updates fetchedAt. Callers that need a guaranteed-fresh
    // fetch after a write should await this completion before invalidating.
    if (playlistsInflight) return playlistsInflight;
    const startEpoch = playlistsMutationEpoch;
    playlistsInflight = (async () => {
        try {
            const { playlists } = await api.getPlaylists();
            // Only overwrite the store if no mutation happened during the
            // in-flight window — otherwise the optimistic update wins.
            if (playlistsMutationEpoch === startEpoch) {
                catalogPlaylists.set(playlists);
            }
            playlistsFetchedAt = Date.now();
            return playlists;
        } finally {
            playlistsInflight = null;
        }
    })();
    return playlistsInflight;
}

export async function ensureCatalogGenres(): Promise<Genre[]> {
    const now = Date.now();
    if (now - genresFetchedAt < TTL_MS && get(catalogGenres).length > 0) {
        return get(catalogGenres);
    }
    // Same in-flight + invalidate caveat as ensureCatalogPlaylists.
    if (genresInflight) return genresInflight;
    const startEpoch = genresMutationEpoch;
    genresInflight = (async () => {
        try {
            const r = await api.getGenres();
            if (genresMutationEpoch === startEpoch) {
                catalogGenres.set(r.genres);
            }
            genresFetchedAt = Date.now();
            return r.genres;
        } finally {
            genresInflight = null;
        }
    })();
    return genresInflight;
}

/**
 * Apply an optimistic mutation. Bumps the mutation epoch so a concurrent
 * in-flight fetch will not overwrite this update on resolution.
 */
export function mutateCatalogPlaylists(fn: (prev: Playlist[]) => Playlist[]) {
    playlistsMutationEpoch++;
    catalogPlaylists.update(fn);
}

export function mutateCatalogGenres(fn: (prev: Genre[]) => Genre[]) {
    genresMutationEpoch++;
    catalogGenres.update(fn);
}

export function invalidateCatalog(which: 'playlists' | 'genres' | 'all' = 'all') {
    if (which === 'playlists' || which === 'all') playlistsFetchedAt = 0;
    if (which === 'genres' || which === 'all') genresFetchedAt = 0;
}

/** Test-only reset. Do not call from app code. */
export function _resetForTest() {
    catalogPlaylists.set([]);
    catalogGenres.set([]);
    playlistsFetchedAt = 0;
    genresFetchedAt = 0;
    playlistsInflight = null;
    genresInflight = null;
    playlistsMutationEpoch = 0;
    genresMutationEpoch = 0;
}
