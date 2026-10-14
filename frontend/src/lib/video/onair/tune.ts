import { get } from 'svelte/store';
import type { VideoChannel } from '$lib/api/video_explore_types';
import { assertOnline } from '$lib/stores/player';
import { showToast } from '$lib/stores/toast';
import { playVideoChannel, videoSession } from '$lib/stores/video_session';
import { exploreMode } from './lineup';

let lastTuneAt = 0;

/** When the last tune-in started: a stage that appears right after one
 *  covers the picture until the video really plays (OnAirStage). */
export function tunedRecently(withinMs: number, now = Date.now()): boolean {
	return now - lastTuneAt < withinMs;
}

/** Tune in from anywhere (the dial, a flip, the guide, an Atlas deep link).
 *  Says so when it cannot, and resolves whether the channel is now on. */
export async function tuneChannel(channel: Pick<VideoChannel, 'id' | 'title'>): Promise<boolean> {
	lastTuneAt = Date.now();
	if (!assertOnline()) {
		showToast('Server is reconnecting.', 'error', 3200);
		return false;
	}
	try {
		const ok = await playVideoChannel(channel.id, { explore: get(exploreMode) });
		// False is a superseded tune (a later flip won, nothing to say) or a
		// stream that failed (the session carries why).
		const error = get(videoSession).error;
		if (!ok && error) showToast(error, 'error', 3200);
		return ok;
	} catch {
		showToast(`Could not tune in to ${channel.title}.`, 'error', 3200);
		return false;
	}
}
