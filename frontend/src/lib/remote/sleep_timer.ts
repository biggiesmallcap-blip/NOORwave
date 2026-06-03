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
// If `fireAt` is more than this far in the past when we discover it, drop it
// without firing. The timer is supposed to fire AT fireAt — if we missed
// that deadline by minutes-to-hours (PWA suspended, then user resumed playback
// from the desktop or another controller), pausing now would be a stale,
// destructive command against an unrelated playback session. The grace covers
// brief screen-off / OS-throttle gaps where firing is still the right thing.
const STALE_GRACE_MS = 120_000;
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

function discardStale() {
	// Drop the persisted timer without firing pausePlayer. Used when we detect
	// the deadline missed its window so long that pausing now would be acting
	// on a different playback session entirely.
	clearHandle();
	const next: SleepTimerState = { fireAt: null, minutes: null };
	sleepTimer.set(next);
	persist(next);
}

function schedule(fireAt: number) {
	clearHandle();
	const delay = fireAt - Date.now();
	if (delay <= 0) {
		// Overdue by a grace window or less → still fire (user's session is
		// probably the same one). Past that, drop silently — see STALE_GRACE_MS.
		if (-delay > STALE_GRACE_MS) {
			discardStale();
			return;
		}
		void fire();
		return;
	}
	handle = setTimeout(() => void fire(), delay);
}

/**
 * Called when the tab returns to foreground. If our timer was supposed to
 * fire while we were suspended, pause immediately. Otherwise reschedule with
 * the remaining wall-clock time so a long suspension doesn't shift the deadline.
 * If overdue by more than STALE_GRACE_MS, discard without firing — a long-
 * dormant timer can't prove the playback session it was set against is still
 * the active one.
 */
function flushIfOverdueOrReschedule() {
	const state = get(sleepTimer);
	if (state.fireAt === null) return;
	const overdue = Date.now() - state.fireAt;
	if (overdue > STALE_GRACE_MS) {
		discardStale();
	} else if (overdue >= 0) {
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

// Boot rehydration: reuse the visibility-resume logic so the stale-grace
// check runs on cold start too (matters when the PWA is killed entirely and
// later relaunched after the deadline passed by hours).
if (typeof window !== 'undefined') {
	ensureVisibilityHandler();
	flushIfOverdueOrReschedule();
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
