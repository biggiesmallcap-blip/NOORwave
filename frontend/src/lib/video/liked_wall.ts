import { api, type LikedVideo, type LikedVideoVersion, type TidalSearchVideo } from '$lib/api/client';

// The liked wall as a queue: one video per song, under the same lens the page
// shows (search, genre, year, order). Shared by the page and by the queue
// extender, which asks again after the page is gone because the background
// scan keeps finding matches long after Play all was pressed.

export interface LikedWallLens {
	query: string;
	genre: string | null;
	year: number | null;
	sort: 'recent' | 'title';
}

/** The video that represents a song on the wall: the best match, longest cut
 *  among equals. Server-sorted, so this is just the head. */
export function likedFace(video: LikedVideo): LikedVideoVersion {
	return video.versions[0];
}

/** Lift a version into the shape the video queue already speaks, so Play all
 *  and Shuffle need no new playback code. */
export function likedQueueItem(video: LikedVideo, version: LikedVideoVersion): TidalSearchVideo {
	return {
		tidal_id: version.tidal_video_id,
		title: version.video_title,
		duration_ms: version.duration_ms,
		artist_id: video.artist_tidal_id,
		artist_name: video.artist_name,
		album_tidal_id: null,
		artwork_url: version.artwork_url,
		quality: null,
		explicit: null,
		type: 'video',
	};
}

export function filterLikedWall(videos: LikedVideo[], lens: LikedWallLens): LikedVideo[] {
	const needle = lens.query.trim().toLowerCase();
	let rows = videos;
	if (needle) {
		rows = rows.filter(
			(v) =>
				v.track_title.toLowerCase().includes(needle) ||
				(v.artist_name ?? '').toLowerCase().includes(needle) ||
				v.versions.some((ver) => ver.video_title.toLowerCase().includes(needle))
		);
	}
	if (lens.genre) rows = rows.filter((v) => v.genre === lens.genre);
	// A card with no album year drops out while a year is ticked: 62% of
	// likes carry one, and an "Unknown" bucket would be a bigger pill than
	// most real years.
	if (lens.year != null) rows = rows.filter((v) => v.album_year === lens.year);

	const sorted = [...rows];
	if (lens.sort === 'title') {
		sorted.sort(
			(a, b) =>
				a.track_title.localeCompare(b.track_title) ||
				(a.artist_name ?? '').localeCompare(b.artist_name ?? '')
		);
	} else {
		sorted.sort((a, b) => (b.liked_at ?? '').localeCompare(a.liked_at ?? ''));
	}
	return sorted;
}

/** One video per song, not every version: queueing six cuts of the same song
 *  back to back is nobody's idea of playing the wall. */
export function likedWallQueue(videos: LikedVideo[], lens: LikedWallLens): TidalSearchVideo[] {
	return filterLikedWall(videos, lens).map((v) => likedQueueItem(v, likedFace(v)));
}

export function shuffled<T>(items: T[]): T[] {
	const out = [...items];
	for (let i = out.length - 1; i > 0; i--) {
		const j = Math.floor(Math.random() * (i + 1));
		[out[i], out[j]] = [out[j], out[i]];
	}
	return out;
}

/** More of the wall for a queue started from it, under the lens of the click.
 *  The session keeps only what it has not queued yet, so this can return the
 *  whole wall; a shuffled queue gets its newcomers shuffled too. */
export function likedWallExtender(lens: LikedWallLens, shuffle: boolean): () => Promise<TidalSearchVideo[]> {
	const frozen = { ...lens };
	return async () => {
		const { videos } = await api.getLikedVideos();
		const queue = likedWallQueue(videos, frozen);
		return shuffle ? shuffled(queue) : queue;
	};
}
