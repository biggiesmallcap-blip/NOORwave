import { writable } from 'svelte/store';

/**
 * Live state of the WASAPI exclusive engagement on the audio engine.
 *
 * Driven by three WS messages from the server:
 * - `audio_exclusive_engaged`  → engaged=true,  failureReason=null
 * - `audio_exclusive_failed`   → engaged=false, failureReason=<msg>   (audio falls back to shared)
 * - `audio_exclusive_released` → engaged=false, failureReason=null    (idle release; will re-grab on next play)
 *
 * The settings page renders a red-pill banner whenever the user has the
 * `exclusive_mode` toggle ON but `engaged` is false AND `failureReason` is
 * non-null — i.e. the user asked for exclusive but the engine couldn't grab
 * it. The released-without-failure state is silent because it's expected.
 */
export interface ExclusiveStatusState {
	engaged: boolean;
	failureReason: string | null;
	device: string | null;
}

export const exclusiveStatus = writable<ExclusiveStatusState>({
	engaged: false,
	failureReason: null,
	device: null,
});

export function setExclusiveEngaged(device: string) {
	exclusiveStatus.set({ engaged: true, failureReason: null, device });
}

export function setExclusiveFailed(device: string, reason: string) {
	exclusiveStatus.set({ engaged: false, failureReason: reason, device });
}

export function setExclusiveReleased(device: string) {
	// Idle release is expected; clear failureReason too so the banner doesn't
	// linger from a previous failure state. Engaged becomes false so the UI
	// knows the engine is currently routing through shared.
	exclusiveStatus.update((s) => ({
		engaged: false,
		failureReason: null,
		device: s.device ?? device,
	}));
}
