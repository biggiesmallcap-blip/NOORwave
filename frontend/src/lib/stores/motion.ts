// App-wide motion preference (Settings > Appearance > Reduce motion).
// 'system' follows the OS accessibility setting (prefers-reduced-motion);
// 'reduce' always reduces. There is no "force full motion": components gate
// motion with @media (prefers-reduced-motion), which a setting cannot undo.
// The effective state is mirrored to <html data-motion="reduce"> so app.css
// can apply one set of rules, and exposed for JS callers.
import { derived, readable, type Readable } from 'svelte/store';
import { createPersistedStore, oneOf } from './persisted';

export type MotionPreference = 'system' | 'reduce';

const STORAGE_KEY = 'noor.motion';
const QUERY = '(prefers-reduced-motion: reduce)';

export function parseMotionPreference(value: unknown): MotionPreference {
	return value === 'reduce' ? 'reduce' : 'system';
}

export function resolveReduceMotion(preference: MotionPreference, systemReduces: boolean): boolean {
	return preference === 'reduce' || systemReduces;
}

export const motionPreference = createPersistedStore<MotionPreference>(STORAGE_KEY, 'system', {
	parse: oneOf(['system', 'reduce'] as const),
});

const systemReducesMotion: Readable<boolean> = readable(false, (set) => {
	if (typeof window === 'undefined' || !window.matchMedia) return;
	const mq = window.matchMedia(QUERY);
	set(mq.matches);
	const update = () => set(mq.matches);
	mq.addEventListener('change', update);
	return () => mq.removeEventListener('change', update);
});

export const reduceMotion = derived([motionPreference, systemReducesMotion], ([pref, system]) =>
	resolveReduceMotion(pref, system),
);

let current = false;
reduceMotion.subscribe((value) => {
	current = value;
	if (typeof document !== 'undefined') {
		if (value) document.documentElement.dataset.motion = 'reduce';
		else delete document.documentElement.dataset.motion;
	}
});

/** For code that decides on the spot (smooth scroll, wheel easing). */
export function prefersReducedMotion(): boolean {
	return current;
}
