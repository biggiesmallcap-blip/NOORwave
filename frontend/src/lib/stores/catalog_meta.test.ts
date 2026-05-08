import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('$lib/api/client', () => ({
    api: {
        getPlaylists: vi.fn(async () => ({ playlists: [{ id: 1, name: 'Mock', is_favorite: false }] })),
        getGenres: vi.fn(async () => ({ genres: [{ id: 'rock', name: 'Rock' }] })),
    },
}));

import { api } from '$lib/api/client';
import {
    catalogPlaylists, catalogGenres,
    ensureCatalogPlaylists, ensureCatalogGenres,
    invalidateCatalog,
    _resetForTest,
} from './catalog_meta';

beforeEach(() => {
    vi.clearAllMocks();
    _resetForTest();
});

describe('catalog_meta', () => {
    it('first call fetches; second call within TTL does not', async () => {
        await ensureCatalogPlaylists();
        await ensureCatalogPlaylists();
        expect(api.getPlaylists).toHaveBeenCalledTimes(1);
        expect(get(catalogPlaylists)).toHaveLength(1);
    });

    it('invalidate forces refetch', async () => {
        await ensureCatalogPlaylists();
        invalidateCatalog('playlists');
        await ensureCatalogPlaylists();
        expect(api.getPlaylists).toHaveBeenCalledTimes(2);
    });

    it('genres cache works the same way', async () => {
        await ensureCatalogGenres();
        await ensureCatalogGenres();
        expect(api.getGenres).toHaveBeenCalledTimes(1);
    });

    it('concurrent callers share a single in-flight fetch', async () => {
        const [a, b, c] = await Promise.all([
            ensureCatalogPlaylists(),
            ensureCatalogPlaylists(),
            ensureCatalogPlaylists(),
        ]);
        expect(api.getPlaylists).toHaveBeenCalledTimes(1);
        expect(a).toBe(b);
        expect(b).toBe(c);
    });

    it('failed fetch does not poison cache; next call retries', async () => {
        (api.getPlaylists as unknown as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('boom'));
        await expect(ensureCatalogPlaylists()).rejects.toThrow('boom');
        await ensureCatalogPlaylists();
        expect(api.getPlaylists).toHaveBeenCalledTimes(2);
    });
});
