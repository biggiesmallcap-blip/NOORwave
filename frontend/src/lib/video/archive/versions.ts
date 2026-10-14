// Versions and concerts: ordering a song's cuts, queueing a concert in its
// setlist order, and the small labels both views share. Pure functions.

import type {
	ExploreVideo,
	VideoConcert,
	VideoCut,
	VideoVersion,
} from '$lib/api/video_explore_types';

/** How a cut ranks when the server did not call one best: the record first,
 *  then the stage, the room, the words, and the rest. */
const CUT_RANK: Record<VideoCut, number> = {
	official: 0,
	live: 1,
	session: 2,
	lyric: 3,
	visualizer: 4,
	remix: 5,
	dance: 6,
	longform: 7,
	clip: 8,
	interview: 9,
	trailer: 10,
};

/** Best first, then by cut, then oldest year first (unknown years last), then
 *  the longer cut, then id so the order never shuffles between renders. */
export function orderVersions(versions: readonly VideoVersion[]): VideoVersion[] {
	return [...versions].sort((a, b) => {
		if (a.best !== b.best) return a.best ? -1 : 1;
		const cut = (CUT_RANK[a.cut] ?? 99) - (CUT_RANK[b.cut] ?? 99);
		if (cut !== 0) return cut;
		const ay = a.cut_year ?? Number.POSITIVE_INFINITY;
		const by = b.cut_year ?? Number.POSITIVE_INFINITY;
		if (ay !== by) return ay - by;
		const length = (b.video.duration_ms ?? 0) - (a.video.duration_ms ?? 0);
		if (length !== 0) return length;
		return a.video.tidal_id - b.video.tidal_id;
	});
}

/** A concert in setlist order (the server's order), each video once, and
 *  where to start: the given video, else the top. */
export function concertQueue(
	concert: Pick<VideoConcert, 'videos'>,
	startVideoId: number | null = null
): { queue: ExploreVideo[]; index: number } {
	const seen = new Set<number>();
	const queue = concert.videos.filter((video) => {
		if (seen.has(video.tidal_id)) return false;
		seen.add(video.tidal_id);
		return true;
	});
	const found = startVideoId == null ? -1 : queue.findIndex((video) => video.tidal_id === startVideoId);
	return { queue, index: Math.max(0, found) };
}

/** A queue of a version list with one picked: every version, picked first,
 *  the rest in list order after it. */
export function versionsQueue(versions: readonly VideoVersion[], pickedId: number): ExploreVideo[] {
	const picked = versions.find((version) => version.video.tidal_id === pickedId);
	const rest = versions.filter((version) => version.video.tidal_id !== pickedId).map((version) => version.video);
	return picked ? [picked.video, ...rest] : rest;
}

/** The other cuts worth naming beside the player: one of each kind, in the
 *  given order, leaving out the kind that is playing. */
export function distinctCuts(others: readonly VideoVersion[], playingCut: VideoCut | null): VideoVersion[] {
	const seen = new Set<VideoCut>(playingCut ? [playingCut] : []);
	return others.filter((version) => {
		if (seen.has(version.cut)) return false;
		seen.add(version.cut);
		return true;
	});
}

/** "Live, 2006": a cut's kind and year, short enough for a chip. */
export function cutChipLabel(version: Pick<VideoVersion, 'cut_year'>, cutLabel: string): string {
	return version.cut_year ? `${cutLabel}, ${version.cut_year}` : cutLabel;
}

/** A one-line caption for a version card: its label, plus the cut and the
 *  year only when the label does not already say them ("Official video",
 *  "Live at Croke Park 2006" stay as they are; "The Farewell Tour" becomes
 *  "The Farewell Tour, Live, 2012"). */
export function versionCaption(version: Pick<VideoVersion, 'label' | 'cut' | 'cut_year'>, cutLabel: string): string {
	const label = version.label.trim();
	const lower = label.toLowerCase();
	const parts = label ? [label] : [];
	if (!lower.includes(cutLabel.toLowerCase())) parts.push(cutLabel);
	if (version.cut_year && !lower.includes(String(version.cut_year))) parts.push(String(version.cut_year));
	return parts.join(', ');
}

/** "1:12:04" for long form, "4:07" otherwise; empty when unknown. */
export function runtimeLabel(ms: number | null | undefined): string {
	if (!ms || !Number.isFinite(ms) || ms <= 0) return '';
	const total = Math.round(ms / 1000);
	const hours = Math.floor(total / 3600);
	const minutes = Math.floor((total % 3600) / 60);
	const seconds = total % 60;
	const ss = String(seconds).padStart(2, '0');
	return hours > 0 ? `${hours}:${String(minutes).padStart(2, '0')}:${ss}` : `${minutes}:${ss}`;
}

/** A concert's length in words: "48 min", "1 hr 52 min". */
export function concertLength(totalSeconds: number): string {
	const minutes = Math.round(totalSeconds / 60);
	if (minutes < 60) return `${minutes} min`;
	const hours = Math.floor(minutes / 60);
	const rest = minutes % 60;
	return rest ? `${hours} hr ${rest} min` : `${hours} hr`;
}
