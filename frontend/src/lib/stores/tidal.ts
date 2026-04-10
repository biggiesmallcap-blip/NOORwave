import { writable } from 'svelte/store';
import { getApiBase } from '$lib/api/client';

export const tidalStatus = writable<'disconnected' | 'connecting' | 'connected'>('disconnected');
export const tidalUserId = writable('');
export const syncStatus = writable<'idle' | 'syncing' | 'done'>('idle');
export const syncProgress = writable<number | null>(null);

// Sync metadata
export interface SyncInfo {
	service: string;
	last_sync_at: string;
	auto_sync_daily: boolean;
	last_sync_track_count: number;
	last_sync_album_count: number;
}
export const syncInfo = writable<SyncInfo | null>(null);

export async function loadTidalStatus() {
	try {
		const resp = await fetch(`${getApiBase()}/api/tidal/status`);
		if (!resp.ok) return;
		const data = await resp.json();
		if (data.connected) {
			tidalStatus.set('connected');
			tidalUserId.set(data.user_id);
		} else {
			tidalStatus.set('disconnected');
			tidalUserId.set('');
		}
	} catch {}
}

export async function loadSyncInfo() {
	try {
		const resp = await fetch(`${getApiBase()}/api/sync/info?service=tidal`);
		if (!resp.ok) return;
		const data = await resp.json();
		syncInfo.set(data.sync);
	} catch {}
}

export async function setAutoSyncDaily(enabled: boolean) {
	try {
		const resp = await fetch(`${getApiBase()}/api/sync/auto`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ service: 'tidal', enabled })
		});
		if (resp.ok) {
			loadSyncInfo(); // Refresh
		}
	} catch {}
}

export function handleSyncProgress(progress: number) {
	syncStatus.set('syncing');
	syncProgress.set(Math.max(0, Math.min(100, Math.round(progress * 100))));
}

export function handleSyncComplete() {
	syncStatus.set('done');
	syncProgress.set(100);
	// Refresh sync info to get updated timestamp
	loadSyncInfo();
}
