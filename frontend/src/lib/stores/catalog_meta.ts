import { writable, get } from 'svelte/store';
import { api, type Playlist, type Genre } from '$lib/api/client';

const TTL_MS = 60_000;

export const catalogPlaylists = writable<Playlist[]>([]);
export const catalogGenres = writable<Genre[]>([]);

let playlistsFetchedAt = 0;
let genresFetchedAt = 0;
let playlistsInflight: Promise<Playlist[]> | null = null;
let genresInflight: Promise<Genre[]> | null = null;

export async function ensureCatalogPlaylists(): Promise<Playlist[]> {
    const now = Date.now();
    if (now - playlistsFetchedAt < TTL_MS && get(catalogPlaylists).length > 0) {
        return get(catalogPlaylists);
    }
    if (playlistsInflight) return playlistsInflight;
    playlistsInflight = (async () => {
        try {
            const { playlists } = await api.getPlaylists();
            catalogPlaylists.set(playlists);
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
    if (genresInflight) return genresInflight;
    genresInflight = (async () => {
        try {
            const r = await api.getGenres();
            catalogGenres.set(r.genres);
            genresFetchedAt = Date.now();
            return r.genres;
        } finally {
            genresInflight = null;
        }
    })();
    return genresInflight;
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
}
