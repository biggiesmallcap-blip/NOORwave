import { beforeEach, describe, expect, test, vi } from 'vitest';
import { TidalContentSettings } from './tidalContent.svelte';

const mocks = vi.hoisted(() => ({ get: vi.fn(), set: vi.fn(), invalidate: vi.fn() }));
vi.mock('$lib/api/client', () => ({ api: { getTidalContentSettings: mocks.get, setTidalContentSettings: mocks.set } }));
vi.mock('$lib/cache/ws_events', () => ({ invalidateTidalContentCaches: mocks.invalidate }));

beforeEach(() => vi.resetAllMocks());
describe('TIDAL content preference', () => {
    test('loads the server preference and disables edits until it is known', async () => {
        const settings = new TidalContentSettings();
        await settings.save(true);
        expect(mocks.set).not.toHaveBeenCalled();
        mocks.get.mockResolvedValue({ hide_ai_generated: true });
        await settings.load();
        expect(settings.known).toBe(true);
        expect(settings.enabled).toBe(true);
    });
    test('failed reads can be retried without inventing a saved value', async () => {
        const settings = new TidalContentSettings();
        mocks.get.mockRejectedValueOnce(new Error('offline')).mockResolvedValue({ hide_ai_generated: false });
        await settings.load();
        expect(settings.known).toBe(false);
        expect(settings.error).toContain('Could not read');
        await settings.load();
        expect(settings.known).toBe(true);
        expect(settings.error).toBe('');
    });
    test('rolls back failed saves and invalidates results only on success', async () => {
        const settings = new TidalContentSettings();
        mocks.get.mockResolvedValue({ hide_ai_generated: false });
        await settings.load();
        mocks.set.mockRejectedValueOnce(new Error('offline')).mockResolvedValue({ hide_ai_generated: true });
        await settings.save(true);
        expect(settings.enabled).toBe(false);
        expect(settings.error).toContain('Could not save');
        expect(mocks.invalidate).not.toHaveBeenCalled();
        await settings.save(true);
        expect(settings.enabled).toBe(true);
        expect(settings.error).toBe('');
        expect(mocks.invalidate).toHaveBeenCalledOnce();
    });
    test('prevents overlapping writes', async () => {
        const settings = new TidalContentSettings();
        mocks.get.mockResolvedValue({ hide_ai_generated: false });
        await settings.load();
        let finish!: (value: { hide_ai_generated: boolean }) => void;
        mocks.set.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
        const saving = settings.save(true);
        await settings.save(false);
        expect(mocks.set).toHaveBeenCalledOnce();
        expect(settings.busy).toBe(true);
        finish({ hide_ai_generated: true });
        await saving;
        expect(settings.busy).toBe(false);
        expect(settings.enabled).toBe(true);
    });
});
