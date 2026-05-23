import { writable, type Readable, derived } from 'svelte/store';

const message = writable<string>('');

let clearHandle: ReturnType<typeof setTimeout> | null = null;
let coalesceHandle: ReturnType<typeof setTimeout> | null = null;
let coalescedResolvedCount = 0;

const COALESCE_WINDOW_MS = 1500;
const CLEAR_AFTER_MS = 2500;

function publish(text: string) {
	if (clearHandle) {
		clearTimeout(clearHandle);
		clearHandle = null;
	}
	// Force the live region to re-fire even when the new text equals the previous
	// value: blank it for a microtask, then set. Some screen readers ignore
	// identical writes without this reset.
	message.set('');
	queueMicrotask(() => {
		message.set(text);
		clearHandle = setTimeout(() => {
			message.set('');
			clearHandle = null;
		}, CLEAR_AFTER_MS);
	});
}

function flushResolved() {
	const n = coalescedResolvedCount;
	coalescedResolvedCount = 0;
	coalesceHandle = null;
	if (n <= 0) return;
	publish(`${n} ${n === 1 ? 'track' : 'tracks'} resolved on TIDAL`);
}

export function announceQueue(text: string) {
	if (!text) return;
	publish(text);
}

export function announceResolved(delta: number) {
	if (delta <= 0) return;
	coalescedResolvedCount += delta;
	if (coalesceHandle) clearTimeout(coalesceHandle);
	coalesceHandle = setTimeout(flushResolved, COALESCE_WINDOW_MS);
}

export const queueAnnouncement: Readable<string> = derived(message, ($m) => $m);

export function _resetForTests() {
	if (clearHandle) clearTimeout(clearHandle);
	if (coalesceHandle) clearTimeout(coalesceHandle);
	clearHandle = null;
	coalesceHandle = null;
	coalescedResolvedCount = 0;
	message.set('');
}
