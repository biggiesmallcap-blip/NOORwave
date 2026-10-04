import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import { loadTidalStatus, tidalStatus } from './tidal';
import {
	pendingTidalLogin, startTidalLogin, completeTidalLogin,
	openTidalVerifyUrl, pasteTidalRedirectUrl, cancelTidalLogin,
} from './tidalLogin';
import { openExternal } from '$lib/util/external';

vi.mock('$lib/util/external', () => ({ openExternal: vi.fn() }));

const verifyUrl = 'https://login.tidal.com/authorize?code_challenge=original';
const redirectUrl = 'https://tidal.com/android/login/auth?code=abc123';
const response = (data: object, status = 200) => new Response(JSON.stringify(data), { status });

describe('pending TIDAL PKCE login', () => {
	beforeEach(() => {
		pendingTidalLogin.set(null);
		tidalStatus.set('disconnected');
		vi.mocked(openExternal).mockResolvedValue({ ok: true, method: 'tauri' });
	});

	afterEach(() => {
		pendingTidalLogin.set(null);
		vi.restoreAllMocks();
		vi.unstubAllGlobals();
	});

	function mockRequests() {
		const fetch = vi.fn(async (url: string) => {
			if (url.endsWith('/login')) return response({ verify_url: verifyUrl });
			if (url.endsWith('/complete')) return response({ user_id: '123' });
			return response({ connected: false });
		});
		vi.stubGlobal('fetch', fetch);
		return fetch;
	}

	test('focus status refresh and remount keep the same pending form and verifier', async () => {
		const fetch = mockRequests();
		await startTidalLogin();
		pendingTidalLogin.update((pending) => ({ ...pending!, redirectUrl }));
		const unsubscribe = pendingTidalLogin.subscribe(() => {});
		unsubscribe(); // Settings leaves the route before returning from the browser.
		await loadTidalStatus(); // Layout refreshes on focus/visibility change.
		expect(get(tidalStatus)).toBe('disconnected');
		let restored: unknown;
		const stop = pendingTidalLogin.subscribe((pending) => { restored = pending; });
		stop();
		expect(restored).toMatchObject({ phase: 'awaiting', verifyUrl, redirectUrl });
		await startTidalLogin(); // A re-login deep link also resumes the same attempt.
		expect(fetch.mock.calls.filter(([url]) => url.endsWith('/login'))).toHaveLength(1);
		await openTidalVerifyUrl();
		expect(openExternal).toHaveBeenLastCalledWith(verifyUrl);
	});

	test('an existing connected account does not dismiss a new pending login', async () => {
		const fetch = mockRequests();
		await startTidalLogin();
		fetch.mockResolvedValueOnce(response({ connected: true, user_id: 'old', auth_flow: 'legacy' }));
		await loadTidalStatus();
		expect(get(tidalStatus)).toBe('connected');
		expect(get(pendingTidalLogin)).toMatchObject({ phase: 'awaiting', verifyUrl });
		cancelTidalLogin();
		expect(get(pendingTidalLogin)).toBeNull();
		expect(get(tidalStatus)).toBe('connected');
	});

	test('completion errors keep the entered URL and allow retry without restarting PKCE', async () => {
		const fetch = mockRequests();
		await startTidalLogin();
		pendingTidalLogin.update((pending) => ({ ...pending!, redirectUrl }));
		fetch.mockResolvedValueOnce(response({ error: 'Temporary TIDAL failure' }, 502));
		await expect(completeTidalLogin()).rejects.toThrow('Temporary TIDAL failure');
		expect(get(pendingTidalLogin)).toMatchObject({
			phase: 'awaiting', verifyUrl, redirectUrl, error: 'Temporary TIDAL failure',
		});
		fetch.mockImplementation(async (url) => url.endsWith('/complete')
			? response({ user_id: '123' })
			: response({ connected: true, user_id: '123', auth_flow: 'pkce' }));
		expect(await completeTidalLogin()).toEqual({ user_id: '123' });
		expect(get(pendingTidalLogin)).toBeNull();
		expect(get(tidalStatus)).toBe('connected');
		expect(fetch.mock.calls.filter(([url]) => url.endsWith('/login'))).toHaveLength(1);
		expect(fetch).toHaveBeenCalledWith(expect.stringContaining('/login/complete'), expect.objectContaining({
			body: JSON.stringify({ redirect_url: redirectUrl }),
		}));
		await loadTidalStatus();
	});

	test('invalid redirect and inaccessible clipboard leave the login available', async () => {
		const fetch = mockRequests();
		await startTidalLogin();
		pendingTidalLogin.update((pending) => ({ ...pending!, redirectUrl: 'invalid' }));
		expect(await completeTidalLogin()).toBeNull();
		expect(fetch).toHaveBeenCalledTimes(1);
		vi.stubGlobal('navigator', { clipboard: { readText: vi.fn().mockRejectedValue(new Error('denied')) } });
		await pasteTidalRedirectUrl();
		expect(get(pendingTidalLogin)).toMatchObject({ phase: 'awaiting', redirectUrl: 'invalid' });
		expect(get(pendingTidalLogin)?.error).toContain('Paste the URL manually');
	});

	test('browser-opening failure preserves the form and manual sign-in link', async () => {
		mockRequests();
		vi.mocked(openExternal).mockResolvedValueOnce({ ok: false, method: 'tauri', error: 'denied' });
		await startTidalLogin();
		expect(get(pendingTidalLogin)).toMatchObject({ phase: 'awaiting', verifyUrl });
		expect(get(pendingTidalLogin)?.externalOpenError).toContain('Copy this TIDAL sign-in link');
	});

	test('network failure during completion keeps the pending form', async () => {
		const fetch = mockRequests();
		await startTidalLogin();
		pendingTidalLogin.update((pending) => ({ ...pending!, redirectUrl }));
		fetch.mockRejectedValueOnce(new TypeError('Failed to fetch'));
		await expect(completeTidalLogin()).rejects.toThrow('Failed to fetch');
		expect(get(pendingTidalLogin)).toMatchObject({ phase: 'awaiting', verifyUrl, redirectUrl });
	});

	test('failed login initiation returns to idle and can be retried', async () => {
		const fetch = mockRequests();
		fetch.mockResolvedValueOnce(response({}, 500));
		await expect(startTidalLogin()).rejects.toThrow('Server returned 500');
		expect(get(pendingTidalLogin)).toBeNull();
		await startTidalLogin();
		expect(get(pendingTidalLogin)?.phase).toBe('awaiting');
	});

	test('duplicate start and finish clicks do not send concurrent auth requests', async () => {
		const fetch = mockRequests();
		let resolveRequest!: (value: Response) => void;
		fetch.mockImplementationOnce(() => new Promise((resolve) => { resolveRequest = resolve; }));
		const start = startTidalLogin();
		await startTidalLogin();
		expect(fetch).toHaveBeenCalledTimes(1);
		resolveRequest(response({ verify_url: verifyUrl }));
		await start;
		pendingTidalLogin.update((pending) => ({ ...pending!, redirectUrl }));
		fetch.mockImplementationOnce(() => new Promise((resolve) => { resolveRequest = resolve; }));
		const complete = completeTidalLogin();
		expect(await completeTidalLogin()).toBeNull();
		cancelTidalLogin(); // Cannot cancel an in-flight exchange.
		expect(get(pendingTidalLogin)?.phase).toBe('completing');
		expect(fetch).toHaveBeenCalledTimes(2);
		resolveRequest(response({ error: 'retry' }, 502));
		await expect(complete).rejects.toThrow('retry');
		cancelTidalLogin();
		expect(get(pendingTidalLogin)).toBeNull();
	});
});
