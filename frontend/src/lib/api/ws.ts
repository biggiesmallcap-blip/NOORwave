import { writable } from 'svelte/store';
import { getApiBase, getStoredToken } from '$lib/api/client';
import { refreshPlaybackState, refreshPlaybackRuntime } from '$lib/stores/player';
import { handleSyncProgress, handleSyncComplete, handleSyncFailed, loadTidalStatus } from '$lib/stores/tidal';
import { handleTrainingProgress, handleTrainingComplete } from '$lib/stores/training';
import { handleAnalysisProgress, handleAnalysisComplete } from '$lib/stores/audio_analysis';
import { handleAcrCloudProgress, handleAcrCloudComplete } from '$lib/stores/acrcloud';
import { handleDiscoverySpaceRefreshed, setRefreshProgress } from '$lib/components/DiscoverSpace/discover_space_store';

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
	| { type: 'sync_failed'; service: string; message: string }
	| { type: 'training_progress'; stage: string; progress: number; message: string; current_track_id: number | null; current_track_title: string | null; tracks_done: number; tracks_total: number }
	| { type: 'audio_analysis_progress'; analyzed: number; total: number; mode: string }
	| { type: 'audio_analysis_complete'; analyzed: number }
	| { type: 'acrcloud_scan_progress'; scanned: number; total: number; matches_found: number }
	| { type: 'acrcloud_scan_complete'; scanned: number; matches_found: number }
	| { type: 'discovery_space_refresh_progress'; seed_track_id: number; stage: string; progress: number }
	| { type: 'discovery_space_refreshed'; seed_track_id: number };

export const wsMessages = writable<WsMessage[]>([]);

let socket: WebSocket | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
let queueRefreshTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleQueueRefresh() {
	if (queueRefreshTimer) clearTimeout(queueRefreshTimer);
	queueRefreshTimer = setTimeout(() => {
		queueRefreshTimer = null;
		void refreshPlaybackState();
	}, 100);
}

function getWebSocketUrl(): string {
	const apiUrl = new URL(getApiBase());
	apiUrl.protocol = apiUrl.protocol === 'https:' ? 'wss:' : 'ws:';
	apiUrl.pathname = '/ws';
	apiUrl.hash = '';
	const token = getStoredToken();
	if (token) {
		apiUrl.searchParams.set('token', token);
	}
	return apiUrl.toString();
}

export function connectWebSocket() {
	// Skip if a socket is already up OR currently connecting — avoids creating
	// a second socket while the first is mid-handshake.
	if (socket?.readyState === WebSocket.OPEN || socket?.readyState === WebSocket.CONNECTING) return;

	socket = new WebSocket(getWebSocketUrl());

	socket.onopen = () => {
		wsConnected.set(true);
		console.log('WebSocket connected');
	};

	socket.onmessage = (event) => {
		try {
			const data = JSON.parse(event.data);
			wsMessages.update((msgs) => [...msgs.slice(-99), data]);
			if (data?.type === 'queue_updated') {
				scheduleQueueRefresh();
			} else if (
				data?.type === 'connected' ||
				data?.type === 'playback_changed' ||
				data?.type === 'track_changed' ||
				data?.type === 'listen_history_updated' ||
				data?.type === 'playback_failed'
			) {
				void refreshPlaybackState();
			}
			if (data?.type === 'track_changed' || data?.type === 'playback_changed') {
				void refreshPlaybackRuntime();
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
			if (data?.type === 'sync_failed' && data?.service === 'tidal') {
				handleSyncFailed(data.message ?? 'TIDAL sync failed');
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
			if (data?.type === 'discovery_space_refresh_progress') {
				setRefreshProgress(data.seed_track_id, data.stage, data.progress);
			}
			if (data?.type === 'discovery_space_refreshed') {
				handleDiscoverySpaceRefreshed(data.seed_track_id);
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
	if (queueRefreshTimer) {
		clearTimeout(queueRefreshTimer);
		queueRefreshTimer = null;
	}
	socket?.close();
	socket = null;
	wsConnected.set(false);
}
