import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { bootstrapRemoteConnection, BoundedRetry, clearRemoteSession, manualPinResponseError, ReconnectScheduler, storePairedSession, takePairingTicket } from './connection';
import { RemoteRequestError } from '$lib/api/remote';

const identity = { server_id: 'server-a', name: 'NOORwave' as const, protocol: 1 as const, pairing_available: true };
const pairing = { token: 'nrp_secret', token_type: 'Bearer' as const, server_id: 'server-a', device: { id: 'phone-1', name: 'Phone', paired_at: '', last_seen_at: null } };

class MemoryStorage implements Storage {
	private values = new Map<string, string>();
	get length() { return this.values.size; }
	clear() { this.values.clear(); }
	getItem(key: string) { return this.values.get(key) ?? null; }
	key(index: number) { return [...this.values.keys()][index] ?? null; }
	removeItem(key: string) { this.values.delete(key); }
	setItem(key: string, value: string) { this.values.set(key, String(value)); }
}

beforeAll(() => { vi.stubGlobal('localStorage', new MemoryStorage()); });

beforeEach(() => {
	localStorage.clear();
	clearRemoteSession();
});

describe('pairing-first bootstrap', () => {
	it('removes the ticket fragment before the only network operation', async () => {
		const order: string[] = [];
		const result = await bootstrapRemoteConnection({
			url: new URL('http://noorwave.local/remote?x=1#pair=ticket-secret'),
			replaceUrl: (url) => order.push(`replace:${url}`),
			identity: vi.fn(async () => { order.push('identity'); return identity; }),
			redeem: vi.fn(async (ticket) => { order.push(`redeem:${ticket}`); return pairing; }),
			probe: vi.fn(),
		});
		expect(order).toEqual(['replace:/remote?x=1', 'redeem:ticket-secret']);
		expect(result).toMatchObject({ phase: 'connected', source: 'paired' });
	});

	it('checks identity before sending a stored credential and clears a mismatch', async () => {
		storePairedSession(pairing);
		const probe = vi.fn();
		const result = await bootstrapRemoteConnection({
			url: new URL('http://host/remote'), replaceUrl: vi.fn(), redeem: vi.fn(), probe,
			identity: vi.fn(async () => ({ ...identity, server_id: 'server-b' })),
		});
		expect(result).toEqual({ phase: 'needs-auth', reason: 'identity-mismatch' });
		expect(probe).not.toHaveBeenCalled();
	});

	it('never sends a paired token that has lost its identity metadata', async () => {
		localStorage.setItem('noor_api_token', 'nrp_orphaned');
		const probe = vi.fn();
		const result = await bootstrapRemoteConnection({
			url: new URL('http://host/remote'), replaceUrl: vi.fn(), redeem: vi.fn(), probe,
			identity: vi.fn(async () => identity),
		});
		expect(result).toEqual({ phase: 'needs-auth', reason: 'identity-mismatch' });
		expect(probe).not.toHaveBeenCalled();
	});

	it.each([
		[401, 'PAIRING_INVALID', 'invalid'], [429, 'RATE_LIMITED', 'rate-limited'], [503, 'STORAGE_UNAVAILABLE', 'storage-unavailable']
	])('maps redemption status %s without normal bootstrap', async (status, code, reason) => {
		const result = await bootstrapRemoteConnection({
			url: new URL('http://host/remote#pair=x'), replaceUrl: vi.fn(), identity: vi.fn(), probe: vi.fn(),
			redeem: vi.fn(async () => { throw new RemoteRequestError(status as number, { error: code as never, message: 'safe' }); }),
		});
		expect(result).toMatchObject({ phase: 'pairing-error', reason });
	});

	it('falls back to memory when persistent storage throws', () => {
		const original = localStorage.setItem.bind(localStorage);
		const spy = vi.spyOn(localStorage, 'setItem').mockImplementation((key, value) => {
			if (key === 'noor_api_token') throw new Error('token blocked');
			return original(key, value);
		});
		expect(storePairedSession(pairing)).toBe(false);
		expect(localStorage.getItem('noor_api_token')).toBeNull();
		expect(localStorage.getItem('noor_remote_session_v1')).toBeNull();
		spy.mockRestore();
	});

	it('rejects duplicate pair fragments after removing them and makes no request', async () => {
		const replaceUrl = vi.fn();
		const redeem = vi.fn();
		const result = await bootstrapRemoteConnection({
			url: new URL('http://host/remote#pair=one&pair=two'), replaceUrl, redeem,
			identity: vi.fn(), probe: vi.fn(),
		});
		expect(replaceUrl).toHaveBeenCalledWith('/remote');
		expect(redeem).not.toHaveBeenCalled();
		expect(result).toMatchObject({ phase: 'pairing-error', reason: 'invalid' });
	});

	it('reports an identity outage separately and retains the paired credential', async () => {
		storePairedSession(pairing);
		const result = await bootstrapRemoteConnection({
			url: new URL('http://host/remote'), replaceUrl: vi.fn(), redeem: vi.fn(), probe: vi.fn(),
			identity: vi.fn(async () => { throw new TypeError('offline'); }),
		});
		expect(result).toMatchObject({ phase: 'network-unavailable', retryable: true });
		expect(localStorage.getItem('noor_api_token')).toBe('nrp_secret');
	});

	it('treats a non-auth probe failure as unavailable and retains the paired credential', async () => {
		storePairedSession(pairing);
		const result = await bootstrapRemoteConnection({
			url: new URL('http://host/remote'), replaceUrl: vi.fn(), redeem: vi.fn(),
			identity: vi.fn(async () => identity),
			probe: vi.fn(async () => new Response(null, { status: 503 })),
		});
		expect(result).toMatchObject({ phase: 'network-unavailable', retryable: true });
		expect(localStorage.getItem('noor_api_token')).toBe('nrp_secret');
	});

	it('times out a hung identity call after five seconds', async () => {
		vi.useFakeTimers();
		const pending = bootstrapRemoteConnection({
			url: new URL('http://host/remote'), replaceUrl: vi.fn(), redeem: vi.fn(), probe: vi.fn(),
			identity: vi.fn(() => new Promise<typeof identity>(() => {})),
		});
		await vi.advanceTimersByTimeAsync(5000);
		await expect(pending).resolves.toMatchObject({ phase: 'network-unavailable', retryable: true });
		vi.useRealTimers();
	});
});

it('only removes the pair parameter and preserves an unrelated fragment', () => {
	const replaced = vi.fn();
	expect(takePairingTicket(new URL('http://host/remote#pair=secret&view=queue'), replaced)).toBe('secret');
	expect(replaced).toHaveBeenCalledWith('/remote#view=queue');
});

it('uses one reconnect timer, bounded backoff, and complete cleanup', () => {
	vi.useFakeTimers();
	const connect = vi.fn();
	const scheduler = new ReconnectScheduler(connect);
	scheduler.next();
	scheduler.next();
	vi.advanceTimersByTime(1999);
	expect(connect).not.toHaveBeenCalled();
	vi.advanceTimersByTime(1);
	expect(connect).toHaveBeenCalledOnce();
	scheduler.next();
	scheduler.clear();
	vi.runAllTimers();
	expect(connect).toHaveBeenCalledOnce();
	vi.useRealTimers();
});

it('bounds explicit connection retries until a successful reset', () => {
	const retries = new BoundedRetry(2);
	expect(retries.tryBegin()).toBe(true);
	expect(retries.tryBegin()).toBe(true);
	expect(retries.tryBegin()).toBe(false);
	retries.reset();
	expect(retries.tryBegin()).toBe(true);
});

it.each([[200, null], [401, 'PIN rejected'], [403, 'PIN rejected'], [429, 'Too many attempts'], [500, 'could not verify']])(
	'handles manual PIN response status %s', (status, expected) => {
		const message = manualPinResponseError(status as number);
		expect(message).toEqual(expected === null ? null : expect.stringContaining(expected as string));
	}
);
