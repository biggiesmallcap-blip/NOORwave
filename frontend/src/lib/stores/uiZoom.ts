import { writable, get } from 'svelte/store';
import { setWebviewZoom } from '$lib/tauri/webview_zoom';

const STORAGE_KEY = 'noor-ui-zoom';
export const MIN = 0.5;
export const MAX = 2.0;
export const DEFAULT = 1.0;
export const WHEEL_STEP = 0.05;
export const KEY_STEP = 0.10;

function clamp(value: number): number {
	if (!Number.isFinite(value)) return DEFAULT;
	if (value < MIN) return MIN;
	if (value > MAX) return MAX;
	// Snap to 2-decimal precision so wheel steps don't accumulate float drift
	// (e.g. 1.00 + 0.05 + 0.05 + … → 1.0500000000000003).
	return Math.round(value * 100) / 100;
}

function readInitial(): number {
	if (typeof localStorage === 'undefined') return DEFAULT;
	const raw = localStorage.getItem(STORAGE_KEY);
	if (raw == null) return DEFAULT;
	const parsed = parseFloat(raw);
	return Number.isFinite(parsed) ? clamp(parsed) : DEFAULT;
}

export const uiZoom = writable<number>(readInitial());

export async function applyZoom(factor: number): Promise<void> {
	const value = clamp(factor);
	await setWebviewZoom(value);
}

export function setZoom(factor: number): void {
	const value = clamp(factor);
	uiZoom.set(value);
	if (typeof localStorage !== 'undefined') {
		localStorage.setItem(STORAGE_KEY, String(value));
	}
	void applyZoom(value);
}

export function zoomIn(): void {
	setZoom(get(uiZoom) + KEY_STEP);
}

export function zoomOut(): void {
	setZoom(get(uiZoom) - KEY_STEP);
}

export function nudgeZoom(deltaSteps: number): void {
	setZoom(get(uiZoom) + deltaSteps * WHEEL_STEP);
}

export function resetZoom(): void {
	setZoom(DEFAULT);
}
