import { writable } from 'svelte/store';
import { getApiBase } from '$lib/api/client';
import { refreshPlaybackState } from '$lib/stores/player';
import { handleSyncProgress, handleSyncComplete, loadTidalStatus } from '$lib/stores/tidal';

export const wsConnected = writable(false);

export type WsMessage =
	| { type: 'connected' }
	| { type: 'playback_changed' }
	| { type: 'track_changed'; track_id: number }
	| { type: 'queue_updated' }
	| { type: 'listen_history_updated'; track_id: number }
	| { type: 'playback_failed'; message: string }
	| { type: 'library_synced' }
	| { type: 'musicbrainz_enriched' }
	| { type: 'sync_progress'; service: string; progress: number };

export const wsMessages = writable<WsMessage[]>([]);

let socket: WebSocket | null = null;
let reconnectTimer: ReturnType<typeof setTimeout>;

function getWebSocketUrl(): string {
	const apiUrl = new URL(getApiBase());
	apiUrl.protocol = apiUrl.protocol === 'https:' ? 'wss:' : 'ws:';
	apiUrl.pathname = '/ws';
	apiUrl.search = '';
	apiUrl.hash = '';
	return apiUrl.toString();
}

export function connectWebSocket() {
	if (socket?.readyState === WebSocket.OPEN) return;

	socket = new WebSocket(getWebSocketUrl());

	socket.onopen = () => {
		wsConnected.set(true);
		console.log('WebSocket connected');
	};

	socket.onmessage = (event) => {
		try {
			const data = JSON.parse(event.data);
			wsMessages.update((msgs) => [...msgs.slice(-99), data]);
			if (
				data?.type === 'connected' ||
				data?.type === 'playback_changed' ||
				data?.type === 'track_changed' ||
				data?.type === 'queue_updated' ||
				data?.type === 'listen_history_updated' ||
				data?.type === 'playback_failed'
			) {
				void refreshPlaybackState();
			}
			if (data?.type === 'connected') {
				void loadTidalStatus();
			}
			if (data?.type === 'sync_progress' && data?.service === 'tidal') {
				handleSyncProgress(data.progress ?? 0);
			}
			if (data?.type === 'library_synced') {
				handleSyncComplete();
			}
		} catch {}
	};

	socket.onclose = () => {
		wsConnected.set(false);
		// Reconnect after 3s
		reconnectTimer = setTimeout(connectWebSocket, 3000);
	};

	socket.onerror = () => {
		socket?.close();
	};
}

export function disconnectWebSocket() {
	clearTimeout(reconnectTimer);
	socket?.close();
	socket = null;
}
