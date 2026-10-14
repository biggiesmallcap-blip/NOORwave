import { writable } from 'svelte/store';

// How a tune-in starts: live joins the channel where its clock is (the TV
// way), from the start begins the video on now at 0:00. A per-device
// preference, so it lives in this browser's storage.

export type TuneMode = 'live' | 'start';

export const TUNE_MODES: readonly { value: TuneMode; label: string; hint: string }[] = [
	{ value: 'live', label: 'Live', hint: 'Join each channel where it is, like TV.' },
	{ value: 'start', label: 'From the start', hint: 'Start the video on now from the beginning.' },
];

const KEY = 'noor.onair.tune.v1';

function read(): TuneMode {
	try {
		return localStorage.getItem(KEY) === 'start' ? 'start' : 'live';
	} catch {
		return 'live';
	}
}

const store = writable<TuneMode>(typeof window === 'undefined' ? 'live' : read());
export const tuneMode = { subscribe: store.subscribe };

export function setTuneMode(mode: TuneMode): void {
	store.set(mode);
	try {
		localStorage.setItem(KEY, mode);
	} catch {
		// Full or blocked storage: the choice holds for this visit.
	}
}
