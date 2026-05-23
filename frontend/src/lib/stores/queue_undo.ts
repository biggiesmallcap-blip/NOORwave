import { writable, get } from 'svelte/store';
import type { QueueItem } from '$lib/api/client';

/**
 * Pending undo offer surfaced by `clearQueue`. Both the desktop layout and
 * the `/remote` queue subscribe to render the affordance, so the undo lives
 * at the store boundary instead of being wired per-call-site.
 *
 * `expiresAt` is a wall-clock millisecond stamp so UIs can render a
 * countdown without holding their own timer. The store auto-clears at the
 * TTL.
 */
export interface PendingUndo {
	count: number;
	items: QueueItem[];
	expiresAt: number;
}

export const pendingUndo = writable<PendingUndo | null>(null);

let clearHandle: ReturnType<typeof setTimeout> | null = null;

export function offerUndo(items: QueueItem[], ttlMs: number = 6000): void {
	if (clearHandle) {
		clearTimeout(clearHandle);
		clearHandle = null;
	}
	if (items.length === 0) {
		pendingUndo.set(null);
		return;
	}
	pendingUndo.set({ count: items.length, items, expiresAt: Date.now() + ttlMs });
	clearHandle = setTimeout(() => {
		pendingUndo.set(null);
		clearHandle = null;
	}, ttlMs);
}

export function consumeUndo(): QueueItem[] | null {
	const current = get(pendingUndo);
	if (!current) return null;
	pendingUndo.set(null);
	if (clearHandle) {
		clearTimeout(clearHandle);
		clearHandle = null;
	}
	return current.items;
}

export function dismissUndo(): void {
	if (clearHandle) {
		clearTimeout(clearHandle);
		clearHandle = null;
	}
	pendingUndo.set(null);
}

export function _resetForTests(): void {
	if (clearHandle) clearTimeout(clearHandle);
	clearHandle = null;
	pendingUndo.set(null);
}
