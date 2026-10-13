// Playback entry points for the Archive, Versions, concerts and Stage rows.
// Thin wrappers over video_session.playVideo so every surface queues the same
// way and says where the queue came from.

import type { ExploreVideo, VideoConcert, VideoVersion } from '$lib/api/video_explore_types';
import { playVideo } from '$lib/stores/video_session';
import { showToast } from '$lib/stores/toast';
import { concertQueue, versionsQueue } from './versions';

async function start(video: ExploreVideo, queue: ExploreVideo[], sourceLabel: string, autoplay?: boolean) {
	const ok = await playVideo(video, {
		queue,
		source: 'search',
		sourceLabel,
		...(autoplay === undefined ? {} : { autoplay }),
	});
	if (!ok) showToast('This video could not be loaded.', 'error', 3200);
	return ok;
}

/** Play one video with a visible list queued behind it. */
export function playFromList(video: ExploreVideo, list: ExploreVideo[], sourceLabel: string) {
	const queue = list.some((item) => item.tidal_id === video.tidal_id) ? list : [video, ...list];
	return start(video, queue, sourceLabel);
}

/** A whole concert, in setlist order, from the top or from one song; it
 *  always autoplays through, because a concert is meant to run on. */
export function playConcert(concert: VideoConcert, startVideoId: number | null = null) {
	const { queue, index } = concertQueue(concert, startVideoId);
	if (queue.length === 0) return Promise.resolve(false);
	return start(queue[index], queue, concert.label, true);
}

/** One version, with the song's other versions queued after it. */
export function playVersion(version: VideoVersion, versions: readonly VideoVersion[], songTitle: string) {
	const queue = versionsQueue(versions, version.video.tidal_id);
	return start(version.video, queue, `Versions of ${songTitle}`);
}
