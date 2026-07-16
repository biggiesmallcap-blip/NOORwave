import { describe, it, expect } from 'vitest';
import { nextToggleAction, toggleLatchShouldRelease } from './player';

// Regression: the play/pause latch used to be released only when the finishing
// toggle was still the LATEST playback intent. Any other intent (a skip / next /
// play) bumps that sequence, so hitting skip while a pause was in flight left
// the latch stranded forever. A stuck latch stops the button reading isPlaying
// and just alternates off a stale value, so the next press sends a no-op
// ("resume" while already playing) and the transport feels dead - exactly the
// "pause unresponsive right after a track change" report.

describe('nextToggleAction', () => {
	it('reads the live store when no toggle is in flight', () => {
		expect(nextToggleAction(null, true)).toBe('pause');
		expect(nextToggleAction(null, false)).toBe('resume');
	});

	it('alternates off the in-flight intent so rapid clicks do not invert', () => {
		// This is the latch's whole purpose: isPlaying lags while responses land,
		// so a second click must alternate off what was just requested.
		expect(nextToggleAction('pause', true)).toBe('resume');
		expect(nextToggleAction('resume', false)).toBe('pause');
	});

	it('ignores a stale store value while a toggle is in flight', () => {
		// isPlaying still says "playing" because the pause response has not
		// landed; the latch must win, not the stale store.
		expect(nextToggleAction('pause', true)).toBe('resume');
	});
});

describe('toggleLatchShouldRelease', () => {
	it('releases when this toggle still owns the latch', () => {
		expect(toggleLatchShouldRelease('pause', 'pause')).toBe(true);
		expect(toggleLatchShouldRelease('resume', 'resume')).toBe(true);
	});

	it('does not release when a newer toggle has taken the latch', () => {
		expect(toggleLatchShouldRelease('resume', 'pause')).toBe(false);
	});

	it('releases even though a racing track change bumped the intent sequence', () => {
		// The bug: release used to also require "still the latest playback
		// intent". A skip bumps that sequence, so this returned false and the
		// latch leaked. Ownership must depend only on the latch itself, so a
		// racing skip can no longer strand it.
		expect(toggleLatchShouldRelease('pause', 'pause')).toBe(true);
	});

	it('is a no-op when the latch is already clear', () => {
		expect(toggleLatchShouldRelease(null, 'pause')).toBe(false);
	});
});

describe('the stuck-latch failure mode is gone', () => {
	it('a released latch lets the next press read real playback state again', () => {
		// Press pause while playing.
		const intended = nextToggleAction(null, true);
		expect(intended).toBe('pause');
		// A skip races here (previously stranded the latch). Release now happens
		// regardless, so the latch is clear...
		expect(toggleLatchShouldRelease(intended, intended)).toBe(true);
		const latchAfter = null;
		// ...and the next press reads the truth instead of alternating off a
		// stale 'pause' (which would have sent a no-op 'resume' while playing).
		expect(nextToggleAction(latchAfter, true)).toBe('pause');
	});
});
