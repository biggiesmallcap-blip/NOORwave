import { writable, get } from 'svelte/store';
import { pausePlayer } from '$lib/stores/player';

/**
 * Frontend sleep timer. The proper place for this would be the server (so it
 * fires even when the phone is locked and the PWA is suspended), but that
 * needs a new endpoint + persistence. As a halfway step we:
 *
 * 1. Track `fireAt` in localStorage so it survives PWA cold starts.
 * 2. Use `setTimeout` while the tab is live for prompt firing.
 * 3. On `visibilitychange` resume (tab returns to foreground), check if
 *    `fireAt` has elapsed during the suspension and pause immediately if so.
 *
 * This covers the most common failure mode — phone locks, JS engine throttles
 * or suspends, the timer would have missed its window — without needing a
 * backend round-trip. It does NOT cover the "tab killed entirely" case; for
 * that we'd need server-side scheduling.
 */
export interface SleepTimerState {
	/** Epoch ms when the timer will fire, or null when idle. */
	fireAt: number | null;
	/** Last selected duration in minutes — used by the pill label and reset. */
	minutes: number | null;
}

const STORAGE_KEY = 'noor.remote.sleepTimer';
const initial: SleepTimerState = { fireAt: null, minutes: null };

function readPersisted(): SleepTimerState {
	if (typeof localStorage === 'undefined') return initial;
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		if (!raw) return initial;
		const parsed = JSON.parse(raw) as Partial<SleepTimerState>;
		const fireAt = typeof parsed.fireAt === 'number' ? parsed.fireAt : null;
		const minutes = typeof parsed.minutes === 'number' ? parsed.minutes : null;
		if (fireAt === null) return initial;
		// Already overdue at startup — don't reschedule, the next user-visible
		// tick will handle it via flushIfOverdue.
		return { fireAt, minutes };
	} catch {
		return initial;
	}
}

function persist(state: SleepTimerState) {
	if (typeof localStorage === 'undefined') return;
	try {
		if (state.fireAt === null) {
			localStorage.removeItem(STORAGE_KEY);
		} else {
			localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
		}
	} catch {
		// Quota / privacy mode — countdown still works in memory.
	}
}

export const sleepTimer = writable<SleepTimerState>(readPersisted());

let handle: ReturnType<typeof setTimeout> | null = null;
let visibilityBound = false;

function clearHandle() {
	if (handle) {
		clearTimeout(handle);
		handle = null;
	}
}

async function fire() {
	clearHandle();
	const next: SleepTimerState = { fireAt: null, minutes: null };
	sleepTimer.set(next);
	persist(next);
	try {
		await pausePlayer();
	} catch {
		// pausePlayer wraps its own error path; swallow here.
	}
}

function schedule(fireAt: number) {
	clearHandle();
	const delay = fireAt - Date.now();
	if (delay <= 0) {
		void fire();
		return;
	}
	handle = setTimeout(() => void fire(), delay);
}

/**
 * Called when the tab returns to foreground. If our timer was supposed to
 * fire while we were suspended, pause immediately. Otherwise reschedule with
 * the remaining wall-clock time so a long suspension doesn't shift the deadline.
 */
function flushIfOverdueOrReschedule() {
	const state = get(sleepTimer);
	if (state.fireAt === null) return;
	if (Date.now() >= state.fireAt) {
		void fire();
	} else {
		schedule(state.fireAt);
	}
}

function ensureVisibilityHandler() {
	if (visibilityBound || typeof document === 'undefined') return;
	document.addEventListener('visibilitychange', () => {
		if (document.visibilityState === 'visible') {
			flushIfOverdueOrReschedule();
		}
	});
	// Also catch pageshow (fired when restored from bfcache on iOS).
	window.addEventListener('pageshow', flushIfOverdueOrReschedule);
	visibilityBound = true;
}

// Boot rehydration: if localStorage already held a future fireAt, reschedule
// it on module load; if it was already overdue, fire as soon as the player
// store is reachable.
if (typeof window !== 'undefined') {
	ensureVisibilityHandler();
	const persisted = get(sleepTimer);
	if (persisted.fireAt !== null) {
		if (Date.now() >= persisted.fireAt) {
			void fire();
		} else {
			schedule(persisted.fireAt);
		}
	}
}

export function startSleepTimer(minutes: number) {
	ensureVisibilityHandler();
	clearHandle();
	const ms = Math.max(1, Math.round(minutes)) * 60_000;
	const fireAt = Date.now() + ms;
	const next: SleepTimerState = { fireAt, minutes };
	sleepTimer.set(next);
	persist(next);
	schedule(fireAt);
}

export function cancelSleepTimer() {
	clearHandle();
	sleepTimer.set(initial);
	persist(initial);
}

/** True iff a timer is currently scheduled. */
export function sleepTimerActive(): boolean {
	return get(sleepTimer).fireAt !== null;
}
