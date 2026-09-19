import { get } from 'svelte/store';
import { createPersistedStore } from '$lib/stores/persisted';

/**
 * Whether the remote should fire `navigator.vibrate` cues on swipe-commit,
 * mode toggles, favourite, sleep-timer, etc. Persists per-device in
 * localStorage so the choice survives page reloads / PWA cold starts.
 *
 * Defaults to ON. Some users find buzz annoying and want it off entirely.
 */
const STORAGE_KEY = 'noor.remote.haptics';

export const hapticsEnabled = createPersistedStore<boolean>(STORAGE_KEY, true, {
	parse: (raw) => raw !== 'off',
	serialize: (on) => (on ? 'on' : 'off'),
});

export function hapticsAreEnabled(): boolean {
	return get(hapticsEnabled);
}

export function toggleHaptics() {
	hapticsEnabled.update((v) => !v);
}
