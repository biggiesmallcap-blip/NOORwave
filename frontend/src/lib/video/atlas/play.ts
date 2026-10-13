// Playing from the Atlas: a sample, an artist's best cuts or a route play as
// a queue in order; radio seeds from one video. Picking a video opens the
// watch page (the section layout follows the stage reveal).

import { get } from 'svelte/store';
import type { AtlasRouteStop, ExploreVideo } from '$lib/api/video_explore_types';
import { playVideo, videoSession } from '$lib/stores/video_session';
import { showToast } from '$lib/stores/toast';

function reportFailure(ok: boolean) {
	if (!ok) showToast(get(videoSession).error ?? 'This video could not be loaded.', 'error', 3200);
}

/** Play `video` with `queue` behind it, through to the end. */
export async function playAtlasQueue(video: ExploreVideo, queue: ExploreVideo[], label: string): Promise<void> {
	const list = queue.length > 0 ? queue : [video];
	const ok = await playVideo(video, {
		queue: list,
		source: 'direct',
		sourceLabel: label,
		autoplay: true,
	});
	reportFailure(ok);
}

/** Endless radio seeded by one of the artist's videos. */
export async function playAtlasRadio(video: ExploreVideo, artistName: string): Promise<void> {
	const ok = await playVideo(video, {
		queue: [video],
		source: 'direct',
		sourceLabel: `${artistName} radio`,
		autoplay: true,
		continuous: true,
		resetRadio: true,
	});
	reportFailure(ok);
}

/** The playable videos of a route, one per stop, in order. */
export function routeQueue(stops: readonly AtlasRouteStop[]): ExploreVideo[] {
	const seen = new Set<number>();
	const out: ExploreVideo[] = [];
	for (const stop of stops) {
		const video = stop.video;
		if (!video || seen.has(video.tidal_id)) continue;
		seen.add(video.tidal_id);
		out.push(video);
	}
	return out;
}

/** "Future to Shakira" for the queue label. */
export function routeLabel(stops: readonly AtlasRouteStop[]): string {
	if (stops.length === 0) return 'Atlas route';
	return `Route: ${stops[0].name} to ${stops[stops.length - 1].name}`;
}

export function formatRuntime(ms: number | null | undefined): string {
	if (!ms || ms <= 0) return '';
	const total = Math.round(ms / 1000);
	const m = Math.floor(total / 60);
	const s = total % 60;
	return `${m}:${String(s).padStart(2, '0')}`;
}

/** Plain-language result of a probe. */
export function probeMessage(name: string, queued: boolean, reason?: string): string {
	if (queued) return `Probe sent. The crawler will look for ${name}'s links and videos next; check back in a few minutes.`;
	const why = (reason ?? '').toLowerCase();
	if (why.includes('off')) return 'Video discovery is off, so probes cannot go out. You can turn it on in Settings.';
	if (why.includes('already')) return `Already checked: TIDAL has no links for ${name} yet.`;
	return reason ? `Not sent: ${reason}.` : 'Not sent right now. Try again later.';
}
