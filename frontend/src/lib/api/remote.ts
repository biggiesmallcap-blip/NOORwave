import { authFetch, getApiBase } from '$lib/api/client';

export type RemoteErrorCode =
	| 'INVALID_REQUEST' | 'PAIRING_INVALID' | 'AUTHENTICATION_REQUIRED' | 'FORBIDDEN'
	| 'REMOTE_NOT_READY' | 'ADDRESS_UNAVAILABLE' | 'DEVICE_LIMIT_REACHED' | 'NOT_FOUND'
	| 'RATE_LIMITED' | 'STORAGE_UNAVAILABLE' | 'DESKTOP_MANAGED' | 'EXTERNAL_BIND_OVERRIDE';

export interface RemoteError { error: RemoteErrorCode; message: string; retry_after_seconds?: number }
export interface RemoteIdentity { server_id: string; name: 'NOORwave'; protocol: 1; pairing_available: boolean }
export interface RemoteAddress { id: string; label: string; url: string; kind: 'lan' | 'other'; recommended: boolean }
export interface RemoteDiagnostic { code: string; message: string }
export interface RemoteStatus {
	server_id: string;
	control: 'desktop' | 'standalone' | 'environment' | 'command_line';
	configured_host_mode: boolean;
	effective_host_mode: boolean;
	restart_required: boolean;
	state: 'disabled' | 'starting' | 'running' | 'unavailable';
	bind_address: string;
	port: number;
	discovery: { state: 'disabled' | 'starting' | 'advertised' | 'unavailable'; hostname: string | null; friendly_url: string | null };
	addresses: RemoteAddress[];
	remote_assets_available: boolean;
	phone_reachability: 'unverified';
	ticket: { id: string; state: 'pending' | 'redeemed' | 'expired'; expires_at: string } | null;
	diagnostics: RemoteDiagnostic[];
}
export interface RemoteDevice { id: string; name: string; paired_at: string; last_seen_at: string | null }
export interface PairingTicketResponse { id: string; pairing_url: string; pairing_code: string; expires_at: string; expires_in_seconds: 120 }
export interface PairingResponse { token: string; token_type: 'Bearer'; server_id: string; device: RemoteDevice }

export class RemoteRequestError extends Error {
	constructor(public status: number, public detail: RemoteError) {
		super(detail.message);
		this.name = 'RemoteRequestError';
	}
}

const VISIBLE_REMOTE_TIMEOUT_MS = 5000;

async function publicRequest<T>(path: string, init: RequestInit = {}): Promise<T> {
	const controller = new AbortController();
	const abort = () => controller.abort(init.signal?.reason);
	if (init.signal?.aborted) abort();
	else init.signal?.addEventListener('abort', abort, { once: true });
	const timer = setTimeout(() => controller.abort(new DOMException('Remote request timed out', 'TimeoutError')), VISIBLE_REMOTE_TIMEOUT_MS);
	try {
		return await fetch(`${getApiBase()}${path}`, { ...init, signal: controller.signal }).then(parse<T>);
	} finally {
		clearTimeout(timer);
		init.signal?.removeEventListener('abort', abort);
	}
}

async function parse<T>(response: Response): Promise<T> {
	if (response.ok) return response.status === 204 ? (undefined as T) : response.json() as Promise<T>;
	let detail: RemoteError = { error: 'INVALID_REQUEST', message: `Request failed (${response.status}).` };
	try { detail = await response.json() as RemoteError; } catch { /* keep safe fallback */ }
	throw new RemoteRequestError(response.status, detail);
}

function management<T>(path: string, init?: RequestInit): Promise<T> {
	return authFetch(`${getApiBase()}${path}`, { ...init, timeoutMs: 5000 }).then(parse<T>);
}

export const remoteApi = {
	identity(signal?: AbortSignal): Promise<RemoteIdentity> {
		return publicRequest<RemoteIdentity>('/api/remote/info', { signal, headers: { accept: 'application/json' } });
	},
	redeem(ticket: string, deviceName?: string, signal?: AbortSignal): Promise<PairingResponse> {
		return publicRequest<PairingResponse>('/api/remote/pair', {
			method: 'POST', signal, headers: { 'content-type': 'application/json', accept: 'application/json' },
			body: JSON.stringify({ ticket, ...(deviceName ? { device_name: deviceName } : {}) })
		});
	},
	status: (signal?: AbortSignal) => management<RemoteStatus>('/api/server/remote', { signal }),
	devices: (signal?: AbortSignal) => management<{ devices: RemoteDevice[] }>('/api/server/remote/devices', { signal }),
	createPairing: (addressId?: string) => management<PairingTicketResponse>('/api/server/remote/pairing', {
		method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(addressId ? { address_id: addressId } : {})
	}),
	cancelPairing: (id: string) => management<void>(`/api/server/remote/pairing/${encodeURIComponent(id)}`, { method: 'DELETE' }),
	renameDevice: (id: string, name: string) => management<RemoteDevice>(`/api/server/remote/devices/${encodeURIComponent(id)}`, {
		method: 'PATCH', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ name })
	}),
	revokeDevice: (id: string) => management<void>(`/api/server/remote/devices/${encodeURIComponent(id)}`, { method: 'DELETE' }),
};
