import { writable } from 'svelte/store';
import { isTauri } from '@tauri-apps/api/core';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { authFetch, getApiBase } from '$lib/api/client';
import { showToast } from '$lib/stores/toast';

/** authFetch takes a full URL (it does not prepend the API base), so build absolute
 *  URLs against the backend origin — a bare `/api/...` would hit the Vite dev origin. */
const api = (path: string) => `${getApiBase()}${path}`;

export type DownloadFormat = 'flac' | 'mp3';
export type FlacQuality = 'cd' | 'hires';

export interface DownloadSettings {
	folder: string;
	format: DownloadFormat;
	flac_quality: FlacQuality;
}

/** The user's default format, mirrored from the server so the player-bar quick
 *  button and the context menus know which format to pre-select. */
export const defaultDownloadFormat = writable<DownloadFormat>('flac');

/** Source tier for lossless (FLAC) downloads: 'cd' (16-bit/44.1kHz) or 'hires'
 *  (best available). Mirrored from the server. */
export const defaultFlacQuality = writable<FlacQuality>('hires');

/** Tracks the user kicked off as single downloads, so the per-item WS event can
 *  raise a "Downloaded / Show in folder" toast (batch items are covered by the
 *  progress toast instead). */
const pendingSingles = new Set<number>();

/** Live progress of the background download worker, mirrored from the server's
 *  WebSocket events. `null` when nothing is downloading. A single pill renders this,
 *  so download progress can never spam the toast stack. */
export interface DownloadProgress {
	done: number;
	total: number;
	currentTitle: string | null;
}
export const downloadProgress = writable<DownloadProgress | null>(null);

function fileNameFromPath(path: string): string {
	const parts = path.split(/[\\/]/);
	return parts[parts.length - 1] || path;
}

async function revealInFolder(path: string): Promise<void> {
	if (!path || !isTauri()) return;
	try {
		await revealItemInDir(path);
	} catch (error) {
		console.warn('revealItemInDir failed', error);
	}
}

// ─── Settings ────────────────────────────────────────────────────────────────

export async function loadDownloadSettings(): Promise<DownloadSettings | null> {
	try {
		const resp = await authFetch(api('/api/downloads/settings'));
		if (!resp.ok) return null;
		const data = (await resp.json()) as DownloadSettings;
		applySettings(data);
		return data;
	} catch {
		return null;
	}
}

function applySettings(data: DownloadSettings): void {
	if (data?.format === 'flac' || data?.format === 'mp3') {
		defaultDownloadFormat.set(data.format);
	}
	if (data?.flac_quality === 'cd' || data?.flac_quality === 'hires') {
		defaultFlacQuality.set(data.flac_quality);
	}
}

export async function saveDownloadSettings(
	patch: { folder?: string; format?: DownloadFormat; flac_quality?: FlacQuality }
): Promise<DownloadSettings | null> {
	try {
		const resp = await authFetch(api('/api/downloads/settings'), {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify(patch)
		});
		if (!resp.ok) {
			showToast("Couldn't save download settings.", 'error', 4000);
			return null;
		}
		const data = (await resp.json()) as DownloadSettings;
		applySettings(data);
		return data;
	} catch {
		showToast("Couldn't save download settings.", 'error', 4000);
		return null;
	}
}

// ─── Triggers ────────────────────────────────────────────────────────────────

export async function downloadTrack(trackId: number, format?: DownloadFormat): Promise<void> {
	pendingSingles.add(trackId);
	try {
		const qs = format ? `?format=${format}` : '';
		const resp = await authFetch(api(`/api/tracks/${trackId}/download${qs}`), {
			method: 'POST',
			timeoutMs: 30_000
		});
		if (!resp.ok) {
			pendingSingles.delete(trackId);
			showToast("Couldn't start the download.", 'error', 4000);
			return;
		}
		const data = await resp.json().catch(() => ({}));
		if (data?.status === 'unavailable') {
			pendingSingles.delete(trackId);
			showToast(data.message ?? "This track can't be downloaded.", 'error', 4500);
		}
		// Otherwise it's queued; completion arrives via the download_item_done event.
	} catch {
		pendingSingles.delete(trackId);
		showToast("Couldn't start the download.", 'error', 4000);
	}
}

export async function downloadTracks(ids: number[], format?: DownloadFormat): Promise<void> {
	if (!ids.length) return;
	// Optimistic: show the pill immediately; the worker's progress events refine it.
	downloadProgress.set({ done: 0, total: ids.length, currentTitle: null });
	try {
		const resp = await authFetch(api('/api/downloads/batch'), {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ ids, format }),
			timeoutMs: 30_000
		});
		if (!resp.ok) {
			downloadProgress.set(null);
			showToast("Couldn't start the downloads.", 'error', 4000);
		}
	} catch {
		downloadProgress.set(null);
		showToast("Couldn't start the downloads.", 'error', 4000);
	}
}

/** Fetch a track list from an endpoint and queue every track for download. Used for
 *  album/playlist batch downloads where the menu only has the container id. */
async function downloadFromEndpoint(path: string, format?: DownloadFormat): Promise<void> {
	try {
		const resp = await authFetch(api(path));
		if (!resp.ok) {
			showToast("Couldn't load the tracks to download.", 'error', 4000);
			return;
		}
		const data = await resp.json();
		const list = Array.isArray(data) ? data : (data.tracks ?? data.items ?? []);
		const ids = list
			.map((t: { id?: number }) => t?.id)
			.filter((id: unknown): id is number => typeof id === 'number');
		if (!ids.length) {
			showToast('Nothing here to download.', 'info', 3000);
			return;
		}
		await downloadTracks(ids, format);
	} catch {
		showToast("Couldn't load the tracks to download.", 'error', 4000);
	}
}

export function downloadAlbum(albumId: number, format?: DownloadFormat): Promise<void> {
	return downloadFromEndpoint(`/api/albums/${albumId}/tracks`, format);
}

export function downloadPlaylist(playlistId: number, format?: DownloadFormat): Promise<void> {
	return downloadFromEndpoint(`/api/playlists/${playlistId}/tracks`, format);
}

export async function cancelDownloads(): Promise<void> {
	try {
		await authFetch(api('/api/downloads/cancel'), { method: 'POST' });
	} catch {
		/* best effort */
	}
}

/** Seed the progress pill from the server so a page reload mid-download recovers the
 *  indicator instead of dropping it. */
export async function refreshDownloadStatus(): Promise<void> {
	try {
		const resp = await authFetch(api('/api/downloads/status'));
		if (!resp.ok) return;
		const status = await resp.json();
		downloadProgress.set(
			status.running
				? { done: status.done, total: status.total, currentTitle: status.current_title ?? null }
				: null
		);
	} catch {
		/* ignore */
	}
}

// ─── WebSocket handlers ───────────────────────────────────────────────────────

export function handleDownloadProgress(data: {
	done: number;
	total: number;
	current_title: string | null;
}): void {
	downloadProgress.set({
		done: data.done,
		total: data.total,
		currentTitle: data.current_title
	});
}

export function handleDownloadItemDone(data: {
	track_id: number;
	ok: boolean;
	already: boolean;
	path: string | null;
	error: string | null;
}): void {
	if (!pendingSingles.has(data.track_id)) return; // batch item; covered by progress/complete
	pendingSingles.delete(data.track_id);

	if (data.ok) {
		const name = data.path ? fileNameFromPath(data.path) : 'track';
		const message = data.already ? `Already downloaded: ${name}` : `Downloaded: ${name}`;
		const actions =
			data.path && isTauri()
				? [{ label: 'Show in folder', onClick: () => void revealInFolder(data.path as string) }]
				: undefined;
		showToast(message, 'success', 5000, actions);
	} else {
		showToast(`Download failed: ${data.error ?? 'unknown error'}`, 'error', 5000);
	}
}

export async function handleDownloadComplete(data: { ok: number; failed: number }): Promise<void> {
	downloadProgress.set(null); // hide the pill
	// A single track is already covered by its own "Downloaded / Show in folder" toast.
	if (data.ok + data.failed <= 1) return;

	let failedIds: number[] = [];
	if (data.failed > 0) {
		try {
			const resp = await authFetch(api('/api/downloads/status'));
			if (resp.ok) {
				const status = await resp.json();
				failedIds = (status.failed ?? []).map((f: { id: number }) => f.id);
			}
		} catch {
			/* ignore */
		}
		const actions = failedIds.length
			? [{ label: 'Retry failed', onClick: () => void downloadTracks(failedIds) }]
			: undefined;
		showToast(
			`Downloaded ${data.ok}, ${data.failed} failed`,
			data.ok > 0 ? 'info' : 'error',
			8000,
			actions
		);
	} else {
		showToast(`Downloaded ${data.ok} ${data.ok === 1 ? 'track' : 'tracks'}`, 'success', 4000);
	}
}
