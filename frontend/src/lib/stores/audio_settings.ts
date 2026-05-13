import { writable, get } from 'svelte/store';
import { api, type AudioSettings } from '$lib/api/client';
import { refreshPlaybackRuntime } from '$lib/stores/player';

export interface AudioSettingsState {
	settings: AudioSettings | null;
	loading: boolean;
	error: string | null;
	pendingApply: boolean;
}

const initial: AudioSettingsState = {
	settings: null,
	loading: false,
	error: null,
	pendingApply: false,
};

function createStore() {
	const { subscribe, update } = writable<AudioSettingsState>(initial);

	return {
		subscribe,
		async load() {
			update((s) => ({ ...s, loading: true, error: null }));
			try {
				const settings = await api.getAudioSettings();
				update((s) => ({ ...s, settings, loading: false }));
			} catch (err) {
				update((s) => ({
					...s,
					loading: false,
					error: err instanceof Error ? err.message : String(err),
				}));
			}
		},
		/**
		 * Merge `patch` into the current settings and PUT the full object.
		 * Optimistically updates the store; reverts on error.
		 */
		async patch(patch: Partial<AudioSettings>) {
			const before = get({ subscribe }).settings;
			if (!before) return;
			const next: AudioSettings = { ...before, ...patch };
			const qualityChanged = next.quality !== before.quality;
			const isLiveApplyChange =
				qualityChanged ||
				next.output_device !== before.output_device ||
				next.exclusive_mode !== before.exclusive_mode ||
				next.sample_rate_follow !== before.sample_rate_follow ||
				next.exclusive_release_grace_secs !== before.exclusive_release_grace_secs ||
				next.exclusive_latency_mode !== before.exclusive_latency_mode;
			update((s) => ({ ...s, settings: next, error: null, pendingApply: isLiveApplyChange }));
			try {
				const saved = await api.updateAudioSettings(next);
				update((s) => ({ ...s, settings: saved, pendingApply: false }));
				if (qualityChanged) {
					void refreshPlaybackRuntime();
				}
			} catch (err) {
				update((s) => ({
					...s,
					settings: before,
					pendingApply: false,
					error: err instanceof Error ? err.message : String(err),
				}));
			}
		},
	};
}

export const audioSettings = createStore();
