// DJ engine state shared by the Mix page: the header switch, the cockpit
// (notice, intent, speed, style, story, feedback) and the Diagnostics
// disclosure all read one status and one poll loop. Mounted views call
// startDjPolling(); the loop runs while at least one of them is mounted.
import { get, writable } from 'svelte/store';
import {
	api,
	type DjMixIntent,
	type DjProfileCorrectionRequest,
	type DjStatusResponse,
	type DjStrategy,
	type DjTransitionSpeedBias,
} from '$lib/api/client';
import { showToast } from '$lib/stores/toast';
import { currentTrack, isPlaying, position } from '$lib/stores/player';
import { cockpitPollInterval, createCockpitRefresh, newlyConfirmedCut } from './cockpit_refresh';

export type DjFeedbackRating = 'good' | 'bad' | 'too_safe' | 'too_bold';

export const djStatus = writable<DjStatusResponse | null>(null);
/** null until the first status arrives. */
export const djEnabled = writable<boolean | null>(null);
export const djMixIntent = writable<DjMixIntent>('balanced');
export const djSpeedBias = writable<DjTransitionSpeedBias>('neutral');
export const djStrategy = writable<DjStrategy>('adaptive');
export const djLoading = writable(true);
export const djSaving = writable(false);
export const djLoadError = writable('');
export const djFiredCut = writable<string | null>(null);
export const djRebuildStatus = writable('');

const cockpitRefresh = createCockpitRefresh(api);
let cutTimer: ReturnType<typeof setTimeout> | undefined;

export async function refreshDj(showLoading = false, force = false) {
	if (showLoading) djLoading.set(true);
	try {
		const snapshot = await cockpitRefresh.refresh(showLoading || force);
		if (!snapshot) return;
		if (snapshot.enabled) djEnabled.set(snapshot.enabled.enabled);
		else if (snapshot.status) djEnabled.set(snapshot.status.enabled);
		if (snapshot.policy) {
			djMixIntent.set(snapshot.policy.mix_intent);
			djSpeedBias.set(snapshot.policy.transition_speed_bias);
			djStrategy.set(snapshot.policy.preferred_strategy ?? 'adaptive');
		}
		if (snapshot.status) {
			const confirmation = newlyConfirmedCut(get(djStatus), snapshot.status);
			if (confirmation) {
				djFiredCut.set(confirmation);
				clearTimeout(cutTimer);
				cutTimer = setTimeout(() => djFiredCut.set(null), 4000);
			}
			djStatus.set(snapshot.status);
		}
		djLoadError.set(snapshot.error);
	} finally {
		djLoading.set(false);
	}
}

let subscribers = 0;
let stopLoop: (() => void) | null = null;

function startLoop(): () => void {
	let disposed = false;
	let timer: ReturnType<typeof setTimeout> | undefined;
	let dueAt = 0;
	const interval = () =>
		cockpitPollInterval(get(djStatus), get(position), get(isPlaying), document.visibilityState === 'visible');
	function schedule(delay: number) {
		if (disposed) return;
		clearTimeout(timer);
		dueAt = Date.now() + delay;
		timer = setTimeout(async () => {
			await refreshDj();
			schedule(interval());
		}, delay);
	}
	function accelerate() {
		const delay = interval();
		if (dueAt > Date.now() + delay) schedule(delay);
	}
	void refreshDj(true).finally(() => schedule(interval()));
	const stopPosition = position.subscribe(accelerate);
	const stopPlaying = isPlaying.subscribe(() => { if (dueAt) schedule(interval()); });
	let trackId = get(currentTrack)?.id;
	const stopTrack = currentTrack.subscribe((track) => {
		if (track?.id !== trackId) {
			trackId = track?.id;
			if (document.visibilityState === 'visible') schedule(200);
		}
	});
	const visibilityChanged = () => schedule(interval());
	document.addEventListener('visibilitychange', visibilityChanged);
	return () => {
		disposed = true;
		clearTimeout(timer);
		clearTimeout(cutTimer);
		stopPosition(); stopPlaying(); stopTrack();
		document.removeEventListener('visibilitychange', visibilityChanged);
		cockpitRefresh.dispose();
	};
}

/** Call from onMount; returns the matching stop for the cleanup. */
export function startDjPolling(): () => void {
	subscribers += 1;
	if (subscribers === 1) stopLoop = startLoop();
	let stopped = false;
	return () => {
		if (stopped) return;
		stopped = true;
		subscribers -= 1;
		if (subscribers === 0) {
			stopLoop?.();
			stopLoop = null;
		}
	};
}

export async function setDjEnabled(next: boolean) {
	if (get(djSaving)) return;
	djSaving.set(true);
	try {
		const response = await api.setDjEnabled(next);
		djEnabled.set(response.enabled);
		await refreshDj(false, true);
	} catch {
		showToast('Could not update DJ engine.', 'error');
	} finally {
		djSaving.set(false);
	}
}

export async function setDjIntent(next: DjMixIntent) {
	djMixIntent.set(next);
	try {
		await api.setDjMixIntent(next);
		await refreshDj(false, true);
	} catch {
		showToast('Could not update mix intent.', 'error');
	}
}

export async function setDjSpeed(next: DjTransitionSpeedBias) {
	djSpeedBias.set(next);
	try {
		await api.setDjPolicy({ transition_speed_bias: next });
		await refreshDj(false, true);
	} catch {
		showToast('Could not update transition speed.', 'error');
	}
}

export async function setDjStrategy(next: DjStrategy) {
	djStrategy.set(next);
	try {
		await api.setDjPolicy({ preferred_strategy: next });
		await refreshDj(false, true);
	} catch {
		showToast('Could not update transition style.', 'error');
	}
}

export async function saveDjCorrection(correction: DjProfileCorrectionRequest) {
	djSaving.set(true);
	try {
		await api.setDjProfileCorrection(correction);
		showToast('DJ correction saved.', 'success');
		await refreshDj(false, true);
	} catch {
		showToast('Could not save DJ correction.', 'error');
	} finally {
		djSaving.set(false);
	}
}

export function clearCorrection(ref: Pick<DjProfileCorrectionRequest, 'media_ref_kind' | 'media_ref_id'>) {
	void saveDjCorrection({
		...ref,
		bpm_multiplier: undefined,
		downbeat_offset_beats: undefined,
		phrase_offset_bars: undefined,
		safe_crossfade_only: false,
		transition_speed_bias: undefined,
		manual_drop_markers_ms: [],
		notes: undefined,
	});
}

export function rebuildProfileStatusMessage(status: string, accepted: boolean) {
	if (accepted && status === 'already_running') return 'Profile rebuild already running';
	if (accepted) return 'Profile rebuild accepted';
	if (status === 'already_current') return 'Profile already current';
	if (status === 'dj_disabled') return 'DJ engine disabled';
	if (status === 'source_unavailable') return 'Profile source unavailable';
	if (status === 'retrying') return 'Profile retrying';
	if (status === 'decode_failed') return 'Profile decode failed';
	return 'Profile is not in the current pair';
}

export async function rebuildProfile(ref: Pick<DjProfileCorrectionRequest, 'media_ref_kind' | 'media_ref_id'>) {
	djRebuildStatus.set('Requesting profile rebuild');
	try {
		const response = await api.rebuildDjProfile(ref);
		djRebuildStatus.set(rebuildProfileStatusMessage(response.status, response.accepted));
		await refreshDj(false, true);
	} catch {
		djRebuildStatus.set('Profile rebuild failed');
		showToast('Could not rebuild DJ profile.', 'error');
	}
}

export async function recordDjFeedback(rating: DjFeedbackRating) {
	try {
		await api.recordDjFeedback({
			transition_event_id: get(djStatus)?.feedback_transition_event_id,
			rating,
		});
		showToast('DJ feedback recorded.', 'success');
		await refreshDj(false, true);
	} catch {
		showToast('Could not record DJ feedback.', 'error');
	}
}

export function acceptSafeOnlySuggestion() {
	const suggestion = get(djStatus)?.safe_crossfade_suggestion;
	if (!suggestion) return;
	void saveDjCorrection({
		media_ref_kind: suggestion.media_ref_kind,
		media_ref_id: suggestion.media_ref_id,
		safe_crossfade_only: true,
		notes: 'Accepted safe-only suggestion',
	});
}
