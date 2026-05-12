import { writable } from 'svelte/store';
import { getApiBase, authFetch } from '$lib/api/client';

export const tidalStatus = writable<'disconnected' | 'connecting' | 'connected'>('disconnected');
export const tidalUserId = writable('');
export const tidalAuthFlow = writable<string | null>(null);
export const tidalPkceClientCredentialSource = writable<string | null>(null);
export const tidalLegacyClientCredentialSource = writable<string | null>(null);
export const syncStatus = writable<'idle' | 'syncing' | 'done' | 'error' | 'cancelled'>('idle');
export const syncProgress = writable<number | null>(null);
export const syncError = writable<string | null>(null);

// Sync metadata
export interface SyncInfo {
	service: string;
	last_sync_at: string;
	auto_sync_daily: boolean;
	last_sync_track_count: number;
	last_sync_album_count: number;
	last_full_sync_at?: string | null;
	last_sync_kind?: string | null;
	tidal_favorite_artist_cursor?: string | null;
	tidal_favorite_album_cursor?: string | null;
	tidal_favorite_track_cursor?: string | null;
}
export const syncInfo = writable<SyncInfo | null>(null);

export type TidalSyncMode = 'auto' | 'full';

export async function loadTidalStatus() {
	try {
		const resp = await authFetch(`${getApiBase()}/api/tidal/status`);
		if (!resp.ok) return;
		const data = await resp.json();
		if (data.connected) {
			tidalUserId.set(data.user_id);
			tidalAuthFlow.set(data.auth_flow ?? null);
			tidalPkceClientCredentialSource.set(data.pkce_client_credential_source ?? null);
			tidalLegacyClientCredentialSource.set(data.legacy_client_credential_source ?? null);
			tidalStatus.set('connected');
		} else {
			tidalUserId.set('');
			tidalAuthFlow.set(null);
			tidalPkceClientCredentialSource.set(null);
			tidalLegacyClientCredentialSource.set(null);
			tidalStatus.set('disconnected');
		}
	} catch {}
}

export async function loadSyncInfo() {
	try {
		const resp = await authFetch(`${getApiBase()}/api/sync/info?service=tidal`);
		if (!resp.ok) return;
		const data = await resp.json();
		syncInfo.set(data.sync);
	} catch {}
}

export async function setAutoSyncDaily(enabled: boolean) {
	try {
		const resp = await authFetch(`${getApiBase()}/api/sync/auto`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ service: 'tidal', enabled })
		});
		if (resp.ok) {
			loadSyncInfo(); // Refresh
		}
	} catch {}
}

export async function cancelTidalSync() {
	try {
		await authFetch(`${getApiBase()}/api/tidal/sync/cancel`, { method: 'POST' });
	} catch {}
}

export async function startTidalSync(mode: TidalSyncMode = 'auto') {
	const suffix = mode === 'full' ? '?mode=full' : '';
	return authFetch(`${getApiBase()}/api/tidal/sync${suffix}`, { method: 'POST' });
}

export function handleSyncProgress(progress: number) {
	syncStatus.set('syncing');
	syncError.set(null);
	syncProgress.set(Math.max(0, Math.min(100, Math.round(progress * 100))));
}

export function handleSyncComplete() {
	syncStatus.set('done');
	syncError.set(null);
	syncProgress.set(100);
	// Refresh sync info to get updated timestamp
	loadSyncInfo();
}

export function handleSyncFailed(message: string) {
	const cancelled = /cancelled/i.test(message);
	syncStatus.set(cancelled ? 'cancelled' : 'error');
	syncError.set(message);
	syncProgress.set(null);
}
