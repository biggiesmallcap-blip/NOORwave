import { writable } from 'svelte/store';
import { getApiBase } from '$lib/api/client';
import { refreshPlaybackState } from '$lib/stores/player';
import { handleSyncProgress, handleSyncComplete, loadTidalStatus } from '$lib/stores/tidal';
import { handleTrainingProgress, handleTrainingComplete } from '$lib/stores/training';
import { handleAnalysisProgress, handleAnalysisComplete } from '$lib/stores/audio_analysis';
import { handleAcrCloudProgress, handleAcrCloudComplete } from '$lib/stores/acrcloud';

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
	| { type: 'sync_progress'; service: string; progress: number }
	| { type: 'training_progress'; stage: string; progress: number; message: string; current_track_id: number | null; current_track_title: string | null; tracks_done: number; tracks_total: number }
	| { type: 'audio_analysis_progress'; analyzed: number; total: number; mode: string }
	| { type: 'audio_analysis_complete'; analyzed: number }
	| { type: 'acrcloud_scan_progress'; scanned: number; total: number; matches_found: number }
	| { type: 'acrcloud_scan_complete'; scanned: number; matches_found: number };

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
			if (data?.type === 'training_progress') {
				handleTrainingProgress(data);
				// Check if this is the final message (evaluate stage at 96%)
				if (data.stage === 'evaluate' && data.progress >= 0.95) {
					// Give a small delay to let the final state settle
					setTimeout(() => handleTrainingComplete(), 500);
				}
			}
			if (data?.type === 'audio_analysis_progress') {
				handleAnalysisProgress(data);
			}
			if (data?.type === 'audio_analysis_complete') {
				handleAnalysisComplete(data);
			}
			if (data?.type === 'acrcloud_scan_progress') {
				handleAcrCloudProgress(data);
			}
			if (data?.type === 'acrcloud_scan_complete') {
				handleAcrCloudComplete(data);
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
