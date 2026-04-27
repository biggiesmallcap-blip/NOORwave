import { writable } from 'svelte/store';

export type ToastKind = 'info' | 'success' | 'error';

export interface Toast {
	id: number;
	message: string;
	kind: ToastKind;
}

export const toasts = writable<Toast[]>([]);

let nextId = 1;
const DEFAULT_TTL_MS = 2200;

export function showToast(message: string, kind: ToastKind = 'info', ttlMs: number = DEFAULT_TTL_MS): number {
	const id = nextId++;
	toasts.update((current) => [...current, { id, message, kind }]);
	setTimeout(() => {
		toasts.update((current) => current.filter((t) => t.id !== id));
	}, ttlMs);
	return id;
}

export function dismissToast(id: number): void {
	toasts.update((current) => current.filter((t) => t.id !== id));
}
