import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ListeningServices, lastfmConnectionLabel } from './listeningServices.svelte';
import { api, type LastfmStatus } from '$lib/api/client';
import { openExternal } from '$lib/util/external';
vi.mock('$lib/api/client', () => ({ api: {
	getLastfmStatus: vi.fn(), getListenBrainzStatus: vi.fn(),
	saveLastfmConfig: vi.fn(), lastfmAuthStart: vi.fn(), lastfmAuthComplete: vi.fn(),
	lastfmAuthDisconnect: vi.fn(), clearLastfmConfig: vi.fn(),
	saveListenBrainzConfig: vi.fn(), clearListenBrainzConfig: vi.fn(),
} }));
vi.mock('$lib/cache/api_queries', () => ({ cacheKeys: { settings: { lastfmStatus: () => 'lfm', listenBrainzStatus: () => 'lb' } }, seedCachedValue: vi.fn() }));
vi.mock('$lib/util/external', () => ({ openExternal: vi.fn() }));
const tags = { api_key_configured: true, api_secret_configured: false, enrichment: true, scrobbling: false, recommendations: false } as LastfmStatus;
beforeEach(() => {
	vi.resetAllMocks();
	vi.mocked(api.getLastfmStatus).mockResolvedValue(tags);
	vi.mocked(api.getListenBrainzStatus).mockResolvedValue({ configured: false, scrobbling: false, recommendations: false, user: null, pending_submissions: 0, failed_submissions: 0 });
});
describe('listening service connection states', () => {
	it('distinguishes tags, saved credentials and confirmed account approval', () => {
		expect(lastfmConnectionLabel(tags)).toBe('Tags ready');
		expect(lastfmConnectionLabel({ ...tags, api_secret_configured: true })).toBe('Credentials saved');
		expect(lastfmConnectionLabel({ ...tags, scrobbling: true })).toBe('Connected');
	});
	it('does not connect an account by merely saving credentials', async () => {
		const services = new ListeningServices();
		vi.mocked(api.saveLastfmConfig).mockResolvedValue({ status: 'ok' });
		await services.saveLastfm(' key ', ' secret ');
		expect(api.saveLastfmConfig).toHaveBeenCalledWith('key', 'secret');
		expect(api.lastfmAuthStart).not.toHaveBeenCalled();
		expect(services.lastfm?.scrobbling).toBe(false);
	});
	it('keeps the approval link usable if opening the external browser fails', async () => {
		const services = new ListeningServices();
		vi.mocked(api.lastfmAuthStart).mockResolvedValue({ status: 'awaiting', auth_url: 'https://www.last.fm/api/auth/' });
		vi.mocked(openExternal).mockResolvedValue({ ok: false, method: 'browser', error: 'Browser unavailable' });
		expect(await services.startLastfm()).toBe(false);
		expect(services.approvalPending).toBe(true);
		expect(services.authUrl).toContain('last.fm');
		expect(services.lastfmError).toContain('approval link');
	});
	it('retains pending approval when account authorisation is incomplete', async () => {
		const services = new ListeningServices(); services.approvalPending = true;
		vi.mocked(api.lastfmAuthComplete).mockResolvedValue({ status: 'not_yet_authorized', message: 'Approve access first.' });
		expect(await services.finishLastfm()).toBe(false);
		expect(services.approvalPending).toBe(true);
		expect(services.lastfmMessage).toBe('');
	});
	it('keeps independent provider status and offers recovery after a failed read', async () => {
		const services = new ListeningServices();
		vi.mocked(api.getLastfmStatus).mockRejectedValue(new Error('Offline'));
		await services.refresh();
		expect(services.lastfm).toBe(null);
		expect(services.lastfmError).toContain('Retry');
		expect(services.listenbrainz?.configured).toBe(false);
	});
	it('does not claim disconnection when the server rejects it', async () => {
		const services = new ListeningServices(); services.lastfm = { ...tags, scrobbling: true };
		vi.mocked(api.lastfmAuthDisconnect).mockRejectedValue(new Error('Offline'));
		expect(await services.disconnectLastfm()).toBe(false);
		expect(services.lastfm?.scrobbling).toBe(true);
		expect(services.lastfmMessage).toBe('');
	});
	it('ignores a stale status request that finishes after a newer connection check', async () => {
		const services = new ListeningServices();
		let finish!: (value: LastfmStatus) => void;
		vi.mocked(api.getLastfmStatus).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
		const stale = services.refresh();
		vi.mocked(api.getLastfmStatus).mockResolvedValue({ ...tags, scrobbling: true });
		await services.refresh();
		finish(tags); await stale;
		expect(services.lastfm?.scrobbling).toBe(true);
		expect(services.loading).toBe(false);
	});
});
