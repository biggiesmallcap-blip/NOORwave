import { describe, it, expect } from 'vitest';
import { shownTrackIsAudible, nowPlayingStatusLabel } from './player';

describe('shownTrackIsAudible', () => {
	it('is true when the runtime confirms the shown track is audible', () => {
		expect(shownTrackIsAudible(42, 42)).toBe(true);
	});

	it('is false when a DIFFERENT track is the one actually producing audio', () => {
		// The "shows a different song playing" desync: a Switch whose new track
		// failed to start leaves the previous engine (id 7) audible while the
		// header optimistically shows the picked track (id 42).
		expect(shownTrackIsAudible(7, 42)).toBe(false);
	});

	it('does not contradict the shown track when the runtime has no opinion', () => {
		// No runtime info yet, or between tracks / cold start: active_track_id
		// is null/undefined, so we must not gate the shown track to "Loading".
		expect(shownTrackIsAudible(null, 42)).toBe(true);
		expect(shownTrackIsAudible(undefined, 42)).toBe(true);
	});

	it('is true when there is no shown track to compare against', () => {
		expect(shownTrackIsAudible(7, null)).toBe(true);
	});
});

describe('nowPlayingStatusLabel', () => {
	const ready = { playerReady: true } as const;

	it('shows Playing only when audio is flowing for the shown track', () => {
		expect(
			nowPlayingStatusLabel({ hasTrack: true, isPlaying: true, audible: true, ...ready })
		).toBe('Playing');
	});

	it('shows Loading (not a false Playing) when a different track is audible', () => {
		expect(
			nowPlayingStatusLabel({ hasTrack: true, isPlaying: true, audible: false, ...ready })
		).toBe('Loading…');
	});

	it('shows Paused when not playing regardless of audibility', () => {
		expect(
			nowPlayingStatusLabel({ hasTrack: true, isPlaying: false, audible: false, ...ready })
		).toBe('Paused');
	});

	it('falls back to Ready / Connecting with no track', () => {
		expect(
			nowPlayingStatusLabel({ hasTrack: false, isPlaying: false, audible: true, playerReady: true })
		).toBe('Ready');
		expect(
			nowPlayingStatusLabel({
				hasTrack: false,
				isPlaying: false,
				audible: true,
				playerReady: false
			})
		).toBe('Connecting');
	});
});
