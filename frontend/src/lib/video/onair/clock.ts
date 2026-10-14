import type { ChannelProgram, VideoChannel } from '$lib/api/video_explore_types';

// On Air runs on a clock: every channel has a schedule, and what you see is
// whatever is on right now. These are the pure sums behind it (no stores, no
// timers), shared by the dial, the tune-in and the stage.

/** Joining the tail of a video is not worth it: past this share of its
 *  runtime a tune-in starts the next scheduled item instead. */
export const SKIP_PAST = 0.8;

const DAY_S = 86_400;

/** Where a tune-in lands. `offsetS` is how far into the scheduled video the
 *  channel is now. Past 80% of a known runtime: skip to the next item. */
export function tuneStart(offsetS: number, durationS: number | null | undefined): { skip: boolean; startAt: number } {
	const offset = Number.isFinite(offsetS) && offsetS > 0 ? offsetS : 0;
	const duration = durationS != null && Number.isFinite(durationS) && durationS > 0 ? durationS : null;
	if (duration == null) return { skip: false, startAt: offset };
	if (offset >= duration * SKIP_PAST) return { skip: true, startAt: 0 };
	return { skip: false, startAt: offset };
}

/** 0..1 through a program; 0 without a runtime. */
export function progressOf(offsetS: number, durationS: number): number {
	if (!Number.isFinite(offsetS) || !Number.isFinite(durationS) || durationS <= 0) return 0;
	return Math.min(1, Math.max(0, offsetS / durationS));
}

export interface LiveSlot {
	/** What is on now, or null when the channel has nothing scheduled. */
	program: ChannelProgram | null;
	/** Seconds into `program`. */
	offset: number;
	progress: number;
	/** What follows `program`, when the lineup still knows. */
	next: ChannelProgram | null;
	/** And what follows that. */
	then: ChannelProgram | null;
	/** The lineup's `now` has run out: the caller should fetch a fresh one. */
	stale: boolean;
}

/** A channel's program `elapsedS` seconds after its lineup was fetched. The
 *  lineup says what was on and how far in; time moves it along, and when
 *  `now` runs out `next` takes over until a refetch lands. */
export function liveSlot(channel: Pick<VideoChannel, 'now' | 'next'> & Partial<Pick<VideoChannel, 'then'>>, elapsedS: number): LiveSlot {
	const now = channel.now;
	const then = channel.then ?? null;
	const elapsed = Number.isFinite(elapsedS) && elapsedS > 0 ? elapsedS : 0;
	if (!now) return { program: null, offset: 0, progress: 0, next: channel.next, then, stale: false };
	const offset = now.offset_s + elapsed;
	if (now.duration_s <= 0 || offset < now.duration_s) {
		return { program: now, offset, progress: progressOf(offset, now.duration_s), next: channel.next, then, stale: false };
	}
	const next = channel.next;
	const into = offset - now.duration_s;
	if (next && (next.duration_s <= 0 || into < next.duration_s)) {
		return { program: next, offset: into, progress: progressOf(into, next.duration_s), next: then, then: null, stale: true };
	}
	// Both ran out: hold the last known picture full until the refetch.
	const last = next ?? now;
	return { program: last, offset: last.duration_s, progress: 1, next: null, then: null, stale: true };
}

/** Long form (a concert, a film) says when it ends rather than how long is left. */
export const LONG_FORM_S = 20 * 60;

/** "For you this morning / this afternoon / tonight" by the local hour. */
export function forYouLabel(secondsSinceMidnight: number): string {
	const hour = Math.floor((((secondsSinceMidnight % 86_400) + 86_400) % 86_400) / 3600);
	if (hour >= 5 && hour < 12) return 'For you this morning';
	if (hour >= 12 && hour < 18) return 'For you this afternoon';
	return 'For you tonight';
}

/** Hours of a locked channel's unlock progress, 0..1. */
export function unlockProgress(unlock: VideoChannel['unlock']): number {
	if (!unlock || !(unlock.need_hours > 0)) return 0;
	return Math.min(1, Math.max(0, unlock.hours / unlock.need_hours));
}

/** Seconds since local midnight. */
export function secondsSinceMidnight(date: Date = new Date()): number {
	return date.getHours() * 3600 + date.getMinutes() * 60 + date.getSeconds() + date.getMilliseconds() / 1000;
}

/** "21:30" for a schedule time in seconds since local midnight. Guides read
 *  in 24 hour time so the column of starts lines up. */
export function formatClock(seconds: number): string {
	const s = ((Math.floor(seconds) % DAY_S) + DAY_S) % DAY_S;
	const h = Math.floor(s / 3600);
	const m = Math.floor((s % 3600) / 60);
	return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}`;
}

/** "4:12", or "1:02:10" for long form. */
export function formatRuntime(seconds: number | null | undefined): string {
	if (seconds == null || !Number.isFinite(seconds) || seconds <= 0) return '';
	const total = Math.round(seconds);
	const h = Math.floor(total / 3600);
	const m = Math.floor((total % 3600) / 60);
	const s = total % 60;
	const ss = String(s).padStart(2, '0');
	return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

/** Channel numbers read like a dial: two digits at least. */
export function channelNumber(no: number): string {
	return String(no).padStart(2, '0');
}
