import { writable } from 'svelte/store';

// Number of frequency bands the backend FFT publishes. Must match
// noor-server::playback::spectrum::NUM_BANDS and the shader's u_spectrum[N].
export const NUM_BANDS = 24;

// Latest real-time spectrum (each band 0..1, low -> high frequency) computed by
// the backend from the audio actually playing, or null when nothing live is
// being analyzed (paused/stopped/idle). Fed from the WebSocket at ~30 Hz.
export const audioSpectrum = writable<number[] | null>(null);

let staleTimer: ReturnType<typeof setTimeout> | null = null;

export function setAudioSpectrum(bands: number[]): void {
	audioSpectrum.set(bands);
	// Frames stop arriving when the audio goes silent. Clear shortly after so
	// the visualiser falls back to its idle state instead of freezing on the
	// last frame.
	if (staleTimer) clearTimeout(staleTimer);
	staleTimer = setTimeout(() => audioSpectrum.set(null), 400);
}
