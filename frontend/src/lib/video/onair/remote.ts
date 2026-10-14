import { get, writable } from 'svelte/store';
import type { OnAirAction } from './keymap';

// On-screen buttons for the remote (the fullscreen channel pill). The stage
// answers a press exactly as it answers the key, so a click flips with the
// same channel card and opens the same guide.

const presses = writable<{ action: OnAirAction; seq: number } | null>(null);

export const remotePresses = { subscribe: presses.subscribe };

export function pressRemote(action: OnAirAction): void {
	presses.set({ action, seq: (get(presses)?.seq ?? 0) + 1 });
}
