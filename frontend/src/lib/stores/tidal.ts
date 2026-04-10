import { writable } from 'svelte/store';
import { getApiBase } from '$lib/api/client';

export const tidalStatus = writable<'disconnected' | 'connecting' | 'connected'>('disconnected');
export const tidalUserId = writable('');
export const syncStatus = writable<'idle' | 'syncing' | 'done'>('idle');
export const syncProgress = writable<number | null>(null);

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

export function handleSyncProgress(progress: number) {
	syncStatus.set('syncing');
	syncProgress.set(Math.max(0, Math.min(100, Math.round(progress * 100))));
}

export function handleSyncComplete() {
	syncStatus.set('done');
	syncProgress.set(100);
}
