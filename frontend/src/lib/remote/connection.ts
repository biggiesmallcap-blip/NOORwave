import { clearPersistedToken, clearStoredToken, getStoredToken, setMemoryToken, setStoredToken } from '$lib/api/client';
import type { PairingResponse, RemoteIdentity } from '$lib/api/remote';
import { RemoteRequestError } from '$lib/api/remote';

const SESSION_KEY = 'noor_remote_session_v1';
export interface RemoteSessionMetadata { server_id: string; device_id: string; name: string }
let memoryMetadata: RemoteSessionMetadata | null = null;

export type BootstrapState =
	| { phase: 'connected'; source: 'paired' | 'stored'; remembered: boolean }
	| { phase: 'needs-auth'; reason: 'missing' | 'identity-mismatch' | 'credential-rejected' }
	| { phase: 'network-unavailable'; retryable: true; message: string }
	| { phase: 'pairing-error'; reason: 'invalid' | 'rate-limited' | 'storage-unavailable' | 'network'; message: string };

export interface BootstrapDependencies {
	url: URL;
	replaceUrl(url: string): void;
	identity(signal?: AbortSignal): Promise<RemoteIdentity>;
	redeem(ticket: string, signal?: AbortSignal): Promise<PairingResponse>;
	probe(token: string, signal?: AbortSignal): Promise<Response>;
}

export class PairingFragmentError extends Error {}

export function takePairingTicket(url: URL, replaceUrl: (url: string) => void): string | null {
	const params = new URLSearchParams(url.hash.startsWith('#') ? url.hash.slice(1) : url.hash);
	const tickets = params.getAll('pair');
	const ticket = tickets[0] ?? null;
	if (!ticket) return null;
	params.delete('pair');
	const clean = `${url.pathname}${url.search}${params.size ? `#${params.toString()}` : ''}`;
	replaceUrl(clean);
	if (tickets.length !== 1) throw new PairingFragmentError('The pairing link is ambiguous. Scan a new QR code.');
	return ticket;
}

export function readRemoteMetadata(): RemoteSessionMetadata | null {
	if (typeof localStorage === 'undefined') return memoryMetadata;
	try {
		const raw = localStorage.getItem(SESSION_KEY);
		if (!raw) return memoryMetadata;
		const value = JSON.parse(raw) as Partial<RemoteSessionMetadata>;
		return typeof value.server_id === 'string' && typeof value.device_id === 'string' && typeof value.name === 'string'
			? value as RemoteSessionMetadata : memoryMetadata;
	} catch { return memoryMetadata; }
}

export function storePairedSession(response: PairingResponse): boolean {
	const metadata = { server_id: response.server_id, device_id: response.device.id, name: response.device.name };
	memoryMetadata = metadata;
	setMemoryToken(response.token);
	if (typeof localStorage === 'undefined') return false;
	clearPersistedToken();
	try {
		localStorage.setItem(SESSION_KEY, JSON.stringify(metadata));
		if (setStoredToken(response.token)) return true;
		localStorage.removeItem(SESSION_KEY);
		return false;
	} catch {
		clearPersistedToken();
		try { localStorage.removeItem(SESSION_KEY); } catch { /* unavailable storage */ }
		return false;
	}
}

export function clearRemoteSession(): void {
	memoryMetadata = null;
	clearStoredToken();
	if (typeof localStorage === 'undefined') return;
	try { localStorage.removeItem(SESSION_KEY); } catch { /* memory is authoritative */ }
}

function pairingFailure(error: unknown): BootstrapState {
	if (error instanceof RemoteRequestError) {
		if (error.status === 429) return { phase: 'pairing-error', reason: 'rate-limited', message: error.message };
		if (error.detail.error === 'PAIRING_INVALID') return { phase: 'pairing-error', reason: 'invalid', message: 'This pairing link expired or was already used.' };
		if (error.detail.error === 'STORAGE_UNAVAILABLE') return { phase: 'pairing-error', reason: 'storage-unavailable', message: 'The computer could not save this phone. Try again.' };
	}
	return { phase: 'pairing-error', reason: 'network', message: 'The computer could not be reached. Check Wi-Fi and try a new QR code.' };
}

const BOOTSTRAP_TIMEOUT_MS = 5000;

async function boundedCall<T>(operation: (signal: AbortSignal) => Promise<T>): Promise<T> {
	const controller = new AbortController();
	const timer = setTimeout(() => controller.abort(), BOOTSTRAP_TIMEOUT_MS);
	try {
		return await Promise.race([
			operation(controller.signal),
			new Promise<T>((_, reject) => controller.signal.addEventListener('abort', () => reject(new DOMException('Connection timed out', 'TimeoutError')), { once: true })),
		]);
	} finally { clearTimeout(timer); }
}

function networkUnavailable(): BootstrapState {
	return { phase: 'network-unavailable', retryable: true, message: 'NOORwave is temporarily unreachable. Check Wi-Fi, then retry.' };
}

export async function bootstrapRemoteConnection(deps: BootstrapDependencies): Promise<BootstrapState> {
	// This is deliberately synchronous and first: no request may observe the URL secret.
	let ticket: string | null;
	try { ticket = takePairingTicket(deps.url, deps.replaceUrl); }
	catch (error) {
		return { phase: 'pairing-error', reason: 'invalid', message: error instanceof Error ? error.message : 'The pairing link is invalid.' };
	}
	if (ticket) {
		try {
			const response = await boundedCall((signal) => deps.redeem(ticket!, signal));
			return { phase: 'connected', source: 'paired', remembered: storePairedSession(response) };
		} catch (error) { return pairingFailure(error); }
	}

	let identity: RemoteIdentity;
	try { identity = await boundedCall((signal) => deps.identity(signal)); } catch { return networkUnavailable(); }
	const token = getStoredToken();
	if (!token) return { phase: 'needs-auth', reason: 'missing' };
	const metadata = readRemoteMetadata();
	if (token.startsWith('nrp_') && !metadata) {
		clearRemoteSession();
		return { phase: 'needs-auth', reason: 'identity-mismatch' };
	}
	if (metadata && metadata.server_id !== identity.server_id) {
		clearRemoteSession();
		return { phase: 'needs-auth', reason: 'identity-mismatch' };
	}
	try {
		const response = await boundedCall((signal) => deps.probe(token, signal));
		if (response.status === 401 || response.status === 403) {
			clearRemoteSession();
			return { phase: 'needs-auth', reason: 'credential-rejected' };
		}
		if (!response.ok) return networkUnavailable();
		return { phase: 'connected', source: 'stored', remembered: true };
	} catch { return networkUnavailable(); }
}

export const RECONNECT_DELAYS_MS = [1000, 2000, 4000, 8000, 15000] as const;

export class BoundedRetry {
	private attempts = 0;
	constructor(private readonly limit = 3) {}
	tryBegin(): boolean {
		if (this.attempts >= this.limit) return false;
		this.attempts += 1;
		return true;
	}
	reset(): void { this.attempts = 0; }
}

export function manualPinResponseError(status: number): string | null {
	if (status >= 200 && status < 300) return null;
	if (status === 401 || status === 403) return 'PIN rejected — double-check the 6 digits.';
	if (status === 429) return 'Too many attempts. Wait a moment before trying again.';
	if (status >= 500) return 'The server could not verify the PIN. Try again.';
	return 'The connection request was not accepted.';
}

export class ReconnectScheduler {
	private timer: ReturnType<typeof setTimeout> | null = null;
	private attempt = 0;
	constructor(private readonly connect: () => void, private readonly schedule = setTimeout, private readonly cancel = clearTimeout) {}
	succeeded(): void { this.attempt = 0; this.clear(); }
	next(): void {
		this.clear();
		const delay = RECONNECT_DELAYS_MS[Math.min(this.attempt, RECONNECT_DELAYS_MS.length - 1)];
		this.attempt += 1;
		this.timer = this.schedule(() => { this.timer = null; this.connect(); }, delay);
	}
	clear(): void { if (this.timer !== null) this.cancel(this.timer); this.timer = null; }
}
