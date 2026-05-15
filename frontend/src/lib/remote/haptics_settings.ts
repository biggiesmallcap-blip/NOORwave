import { writable, get } from 'svelte/store';

/**
 * Whether the remote should fire `navigator.vibrate` cues on swipe-commit,
 * mode toggles, favourite, sleep-timer, etc. Persists per-device in
 * localStorage so the choice survives page reloads / PWA cold starts.
 *
 * Defaults to ON. Some users find buzz annoying and want it off entirely.
 */
const STORAGE_KEY = 'noor.remote.haptics';

function readInitial(): boolean {
	if (typeof localStorage === 'undefined') return true;
	const raw = localStorage.getItem(STORAGE_KEY);
	if (raw === null) return true;
	return raw !== 'off';
}

export const hapticsEnabled = writable<boolean>(readInitial());

hapticsEnabled.subscribe((on) => {
	if (typeof localStorage === 'undefined') return;
	localStorage.setItem(STORAGE_KEY, on ? 'on' : 'off');
});

export function hapticsAreEnabled(): boolean {
	return get(hapticsEnabled);
}

export function toggleHaptics() {
	hapticsEnabled.update((v) => !v);
}
