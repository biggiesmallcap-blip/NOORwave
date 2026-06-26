import { writable } from 'svelte/store';

export type ToastKind = 'info' | 'success' | 'error';

export interface ToastAction {
	label: string;
	onClick: () => void;
}

export interface Toast {
	id: number;
	message: string;
	kind: ToastKind;
	actions?: ToastAction[];
}

export const toasts = writable<Toast[]>([]);

let nextId = 1;
const DEFAULT_TTL_MS = 2200;
const timers = new Map<number, ReturnType<typeof setTimeout>>();

function scheduleDismiss(id: number, ttlMs: number): void {
	const existing = timers.get(id);
	if (existing) clearTimeout(existing);
	if (ttlMs === Infinity) return;
	timers.set(
		id,
		setTimeout(() => {
			dismissToast(id);
		}, ttlMs)
	);
}

export function showToast(
	message: string,
	kind: ToastKind = 'info',
	ttlMs: number = DEFAULT_TTL_MS,
	actions?: ToastAction[]
): number {
	const id = nextId++;
	toasts.update((current) => [...current, { id, message, kind, actions }]);
	scheduleDismiss(id, ttlMs);
	return id;
}

/** Update an existing toast in place (e.g. live batch progress). Resets its TTL. */
export function updateToast(
	id: number,
	patch: Partial<Omit<Toast, 'id'>>,
	ttlMs: number = Infinity
): void {
	toasts.update((current) =>
		current.map((t) => (t.id === id ? { ...t, ...patch } : t))
	);
	scheduleDismiss(id, ttlMs);
}

export function dismissToast(id: number): void {
	const timer = timers.get(id);
	if (timer) {
		clearTimeout(timer);
		timers.delete(id);
	}
	toasts.update((current) => current.filter((t) => t.id !== id));
}
