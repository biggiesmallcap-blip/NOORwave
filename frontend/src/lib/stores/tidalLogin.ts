import { get, writable } from 'svelte/store';
import { authFetch, getApiBase } from '$lib/api/client';
import { isValidTidalRedirectUrl, readTidalRedirectFromClipboard } from '$lib/tidal/login';
import { openExternal } from '$lib/util/external';
import { loadTidalStatus, tidalStatus, tidalUserId } from './tidal';

interface PendingTidalLogin {
	phase: 'starting' | 'awaiting' | 'completing';
	verifyUrl: string;
	redirectUrl: string;
	error: string;
	externalOpenError: string;
}

// Keep the form for the lifetime of this UI session, including route remounts.
// Connection-status refreshes cannot tell whether a PKCE login is still pending.
// Do not persist authorization codes to disk; a full UI reload starts a new form.
export const pendingTidalLogin = writable<PendingTidalLogin | null>(null);

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isValidSignInUrl(value: unknown): value is string {
	if (typeof value !== 'string') return false;
	try {
		const url = new URL(value);
		return url.origin === 'https://login.tidal.com' && url.pathname === '/authorize' &&
			url.username === '' && url.password === '';
	} catch {
		return false;
	}
}

export async function openTidalVerifyUrl() {
	const pending = get(pendingTidalLogin);
	if (!pending?.verifyUrl) return;
	pending.externalOpenError = '';
	pendingTidalLogin.set(pending);
	try {
		const result = await openExternal(pending.verifyUrl);
		if (!result.ok) throw new Error(result.error);
	} catch (error) {
		if (get(pendingTidalLogin) !== pending) return;
		pending.externalOpenError = `Browser did not open: ${error instanceof Error ? error.message : String(error)}. Copy this TIDAL sign-in link into your browser.`;
		pendingTidalLogin.set(pending);
	}
}

export async function startTidalLogin() {
	// Reopening Settings must resume the existing verifier, not replace it.
	if (get(pendingTidalLogin)) return;
	const pending: PendingTidalLogin = {
		phase: 'starting', verifyUrl: '', redirectUrl: '', error: '', externalOpenError: '',
	};
	pendingTidalLogin.set(pending);
	try {
		const resp = await authFetch(`${getApiBase()}/api/tidal/login`, { method: 'POST' });
		if (!resp.ok) throw new Error(`Server returned ${resp.status}`);
		const data: unknown = await resp.json();
		if (!isRecord(data) || !isValidSignInUrl(data.verify_url)) {
			throw new Error('Server did not return a valid TIDAL sign-in URL.');
		}
		pending.verifyUrl = data.verify_url;
		pending.phase = 'awaiting';
		pendingTidalLogin.set(pending);
		await openTidalVerifyUrl();
	} catch (error) {
		pendingTidalLogin.set(null);
		throw error;
	}
}

export async function completeTidalLogin(): Promise<{ user_id: string } | null> {
	const pending = get(pendingTidalLogin);
	if (!pending || pending.phase !== 'awaiting') return null;
	pending.error = '';
	if (!isValidTidalRedirectUrl(pending.redirectUrl)) {
		pending.error = 'Paste the final TIDAL redirect URL to finish login.';
		pendingTidalLogin.set(pending);
		return null;
	}
	pending.phase = 'completing';
	pendingTidalLogin.set(pending);
	try {
		const resp = await authFetch(`${getApiBase()}/api/tidal/login/complete`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ redirect_url: pending.redirectUrl.trim() }),
		});
		const data: unknown = await resp.json().catch(() => null);
		if (!resp.ok) {
			throw new Error(isRecord(data) && typeof data.error === 'string'
				? data.error : `Server returned ${resp.status}`);
		}
		if (!isRecord(data) || data.status !== 'authenticated' ||
			typeof data.user_id !== 'string' || !data.user_id.trim()) {
			throw new Error('Server did not confirm TIDAL authentication. Try finishing login again.');
		}
		tidalUserId.set(data.user_id);
		tidalStatus.set('connected');
		pendingTidalLogin.set(null);
		void loadTidalStatus();
		return { user_id: data.user_id };
	} catch (error) {
		pending.phase = 'awaiting';
		pending.error = error instanceof Error ? error.message : String(error);
		pendingTidalLogin.set(pending);
		throw error;
	}
}

export async function pasteTidalRedirectUrl() {
	const pending = get(pendingTidalLogin);
	if (!pending || pending.phase !== 'awaiting') return;
	const result = await readTidalRedirectFromClipboard();
	if (get(pendingTidalLogin) !== pending || pending.phase !== 'awaiting') return;
	if (result.ok && result.redirectUrl) {
		pending.redirectUrl = result.redirectUrl;
		pending.error = '';
	} else {
		pending.error = result.error ?? 'Clipboard access failed. Paste the URL manually.';
	}
	pendingTidalLogin.set(pending);
}

export function cancelTidalLogin() {
	// Cancel this UI attempt without signing out an existing account. The next
	// login request replaces the backend's in-memory PKCE verifier.
	if (get(pendingTidalLogin)?.phase === 'awaiting') pendingTidalLogin.set(null);
}
