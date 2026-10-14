import type { VideoChannel } from '$lib/api/video_explore_types';

// Channel surfing: which channel Up/Down lands on, how typed digits become a
// channel number, and the debounce that lets a held rocker skim past several
// channels while tuning only the one you stop on.

type Dialable = Pick<VideoChannel, 'id' | 'no' | 'locked'>;

/** Tunable channels in dial order. */
export function dialOrder<T extends Dialable>(channels: readonly T[]): T[] {
	return channels.filter((channel) => !channel.locked).sort((a, b) => a.no - b.no);
}

/** The next tunable channel above (step 1) or below (step -1) `fromNo`,
 *  wrapping round the dial like a TV. A number that is not on the dial
 *  (a locked channel, nothing tuned) starts from where it would sit. */
export function flipTarget<T extends Dialable>(channels: readonly T[], fromNo: number | null, step: 1 | -1): T | null {
	const dial = dialOrder(channels);
	if (dial.length === 0) return null;
	if (fromNo == null) return step === 1 ? dial[0] : dial[dial.length - 1];
	if (step === 1) return dial.find((channel) => channel.no > fromNo) ?? dial[0];
	for (let i = dial.length - 1; i >= 0; i -= 1) if (dial[i].no < fromNo) return dial[i];
	return dial[dial.length - 1];
}

export type DigitVerdict = 'commit' | 'wait' | 'invalid';

/** What a typed number means so far. Commit as soon as it can only be one
 *  channel; wait while a longer number could still follow ("1" while 12
 *  exists); invalid when nothing starts like this. Leading zeros are allowed
 *  ("05" is 5). Three digits always settle it. */
export function digitVerdict(buffer: string, numbers: readonly number[]): DigitVerdict {
	const digits = buffer.replace(/^0+(?=\d)/, '');
	if (!/^\d+$/.test(digits)) return 'invalid';
	const exact = numbers.includes(Number(digits));
	const longer = buffer.length < 3 && numbers.some((no) => {
		const text = String(no);
		return text.length > digits.length && text.startsWith(digits);
	});
	if (longer) return 'wait';
	return exact ? 'commit' : 'invalid';
}

/** How long a half-typed number waits for its next digit. */
export const DIGIT_WAIT_MS = 1400;
/** A held rocker skims channels; the tune waits this long after the last press. */
export const FLIP_SETTLE_MS = 320;

export interface FlipperOptions<T extends Dialable> {
	channels: () => readonly T[];
	/** The channel on air now (number), the start of the first press. */
	currentNo: () => number | null;
	/** Shown at once on every press: the card of where you are going. */
	preview: (channel: T) => void;
	/** Runs once the presses settle. */
	tune: (channel: T) => void;
	settleMs?: number;
	setTimer?: (fn: () => void, ms: number) => unknown;
	clearTimer?: (handle: unknown) => void;
}

/** Debounced channel flips. Each press moves on from the last target (not
 *  from what is on air), previews it instantly, and restarts the settle
 *  timer; only the channel you stop on is tuned. */
export function createFlipper<T extends Dialable>(opts: FlipperOptions<T>) {
	const setTimer = opts.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
	const clearTimer = opts.clearTimer ?? ((handle) => clearTimeout(handle as ReturnType<typeof setTimeout>));
	let target: T | null = null;
	let timer: unknown = null;

	function schedule(channel: T) {
		target = channel;
		opts.preview(channel);
		if (timer != null) clearTimer(timer);
		timer = setTimer(() => {
			timer = null;
			const settled = target;
			target = null;
			if (settled) opts.tune(settled);
		}, opts.settleMs ?? FLIP_SETTLE_MS);
	}

	return {
		press(step: 1 | -1): T | null {
			const from = target?.no ?? opts.currentNo();
			const next = flipTarget(opts.channels(), from, step);
			if (next) schedule(next);
			return next;
		},
		/** Dialled directly (digits, guide): same preview, same settle. */
		go(channel: T) {
			schedule(channel);
		},
		get pending(): T | null {
			return target;
		},
		cancel() {
			if (timer != null) clearTimer(timer);
			timer = null;
			target = null;
		},
	};
}
