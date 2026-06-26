import { writable } from 'svelte/store';
import { isTauri } from '@tauri-apps/api/core';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { authFetch } from '$lib/api/client';
import { showToast, updateToast, dismissToast } from '$lib/stores/toast';

export type DownloadFormat = 'flac' | 'mp3';

export interface DownloadSettings {
	folder: string;
	format: DownloadFormat;
}

/** The user's default format, mirrored from the server so the player-bar quick
 *  button and the context menus know which format to pre-select. */
export const defaultDownloadFormat = writable<DownloadFormat>('flac');

/** Tracks the user kicked off as single downloads, so the per-item WS event can
 *  raise a "Downloaded / Show in folder" toast (batch items are covered by the
 *  progress toast instead). */
const pendingSingles = new Set<number>();

let batchToastId: number | null = null;
let batchActive = false;

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

function endBatchToast(): void {
	if (batchToastId != null) {
		dismissToast(batchToastId);
		batchToastId = null;
	}
}

// ─── Settings ────────────────────────────────────────────────────────────────

export async function loadDownloadSettings(): Promise<DownloadSettings | null> {
	try {
		const resp = await authFetch('/api/downloads/settings');
		if (!resp.ok) return null;
		const data = (await resp.json()) as DownloadSettings;
		if (data?.format === 'flac' || data?.format === 'mp3') {
			defaultDownloadFormat.set(data.format);
		}
		return data;
	} catch {
		return null;
	}
}

export async function saveDownloadSettings(
	patch: { folder?: string; format?: DownloadFormat }
): Promise<DownloadSettings | null> {
	try {
		const resp = await authFetch('/api/downloads/settings', {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify(patch)
		});
		if (!resp.ok) return null;
		const data = (await resp.json()) as DownloadSettings;
		if (data?.format === 'flac' || data?.format === 'mp3') {
			defaultDownloadFormat.set(data.format);
		}
		return data;
	} catch {
		return null;
	}
}

// ─── Triggers ────────────────────────────────────────────────────────────────

export async function downloadTrack(trackId: number, format?: DownloadFormat): Promise<void> {
	pendingSingles.add(trackId);
	try {
		const qs = format ? `?format=${format}` : '';
		const resp = await authFetch(`/api/tracks/${trackId}/download${qs}`, {
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
	batchActive = true;
	if (batchToastId == null) {
		batchToastId = showToast(`Downloading: 0/${ids.length}`, 'info', Infinity, [
			{ label: 'Cancel', onClick: () => void cancelDownloads() }
		]);
	}
	try {
		const resp = await authFetch('/api/downloads/batch', {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ ids, format }),
			timeoutMs: 30_000
		});
		if (!resp.ok) {
			endBatchToast();
			batchActive = false;
			showToast("Couldn't start the downloads.", 'error', 4000);
		}
	} catch {
		endBatchToast();
		batchActive = false;
		showToast("Couldn't start the downloads.", 'error', 4000);
	}
}

/** Fetch a track list from an endpoint and queue every track for download. Used for
 *  album/playlist batch downloads where the menu only has the container id. */
async function downloadFromEndpoint(path: string, format?: DownloadFormat): Promise<void> {
	try {
		const resp = await authFetch(path);
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
		await authFetch('/api/downloads/cancel', { method: 'POST' });
	} catch {
		/* best effort */
	}
}

// ─── WebSocket handlers ───────────────────────────────────────────────────────

export function handleDownloadProgress(data: {
	done: number;
	total: number;
	current_title: string | null;
}): void {
	if (data.total <= 1) return; // singles are handled by per-item toasts
	batchActive = true;
	if (batchToastId == null) {
		batchToastId = showToast('', 'info', Infinity, [
			{ label: 'Cancel', onClick: () => void cancelDownloads() }
		]);
	}
	const label = data.current_title
		? `Downloading ${data.current_title} — ${data.done}/${data.total}`
		: `Downloading: ${data.done}/${data.total}`;
	updateToast(batchToastId, { message: label });
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
	const wasBatch = batchActive;
	endBatchToast();
	batchActive = false;
	if (!wasBatch) return; // singles-only session; per-item toasts already covered it

	let failedIds: number[] = [];
	if (data.failed > 0) {
		try {
			const resp = await authFetch('/api/downloads/status');
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
