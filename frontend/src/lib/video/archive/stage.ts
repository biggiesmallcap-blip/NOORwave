// The Stage: the rows under the watch page's player. Each row comes from the
// Atlas and Air endpoints; any of them may answer 501 (not built yet) or fail.
// The concert and the other cuts sit by the player; the exploring picks (same
// scene, one step out) open the related grid under it.

import type {
	AtlasArtistCard,
	AtlasNeighbor,
	AtlasScene,
	AtlasSceneCard,
	ExploreVideo,
	SongVersionsResponse,
	VideoConcert,
	VideoVersion,
} from '$lib/api/video_explore_types';
import { orderVersions } from './versions';

export interface StageVideo {
	tidal_id: number;
	title: string;
	artist_id: number | null;
	artist_name: string | null;
}

export interface StageClient {
	getVideoSongByVideo(videoId: number): Promise<SongVersionsResponse>;
	getVideoConcert(id: number): Promise<VideoConcert>;
	getVideoAtlasArtist(id: number): Promise<AtlasArtistCard>;
	getVideoAtlasScene(id: number): Promise<AtlasSceneCard>;
}

/** One card in a Stage row. */
export interface StageItem {
	video: ExploreVideo;
	/** Why it is here, under the card. */
	caption?: string | null;
	/** Marks the playing video inside a concert row. */
	current?: boolean;
	/** Setlist position, over the artwork. */
	index?: number | null;
}

export interface StepOutPick {
	video: ExploreVideo;
	neighbor: AtlasNeighbor;
}

export interface StageRows {
	concert: VideoConcert | null;
	scene: { scene: AtlasScene; videos: ExploreVideo[] } | null;
	stepOut: StepOutPick[];
	versions: { song: SongVersionsResponse; others: VideoVersion[] } | null;
}

export const EMPTY_STAGE: StageRows = { concert: null, scene: null, stepOut: [], versions: null };

export const STEP_OUT_LIMIT = 8;
export const SCENE_SAMPLE_LIMIT = 12;

/** Neighbours worth a step: not yet lit or warm (light <= 1), strongest edge
 *  first, one per artist, never the current artist. */
export function stepOutNeighbors(card: AtlasArtistCard, limit = STEP_OUT_LIMIT): AtlasNeighbor[] {
	const seen = new Set<number>([card.id]);
	return [...card.neighbors]
		.filter((neighbor) => neighbor.light <= 1)
		.sort((a, b) => b.weight - a.weight || a.id - b.id)
		.filter((neighbor) => {
			if (seen.has(neighbor.id)) return false;
			seen.add(neighbor.id);
			return true;
		})
		.slice(0, limit);
}

/** Where a neighbour edge came from, in words. */
export function viaLabel(source: string): string {
	switch (source) {
		case 'tidal': return 'TIDAL related';
		case 'lastfm': return 'Last.fm similar';
		case 'featured': return 'Featured together';
		case 'colist': return 'Listed together';
		default: return 'Nearby';
	}
}

function hasVideo(song: SongVersionsResponse, videoId: number): boolean {
	return song.versions.some((version) => version.video.tidal_id === videoId);
}

/** The playing video's song, looked up by the video itself on the server
 *  (one source of truth for song keys). Null when the catalog has not been
 *  built yet or the video is unknown. */
export async function findSong(client: StageClient, video: StageVideo): Promise<SongVersionsResponse | null> {
	try {
		const song = await client.getVideoSongByVideo(video.tidal_id);
		return hasVideo(song, video.tidal_id) ? song : null;
	} catch {
		return null;
	}
}

function status(error: unknown): number | null {
	const value = (error as { status?: unknown } | null)?.status;
	return typeof value === 'number' ? value : null;
}

const ARTIST_TTL_MS = 5 * 60_000;
const artistCache = new Map<number, { at: number; card: Promise<AtlasArtistCard> }>();
const sceneCache = new Map<number, { at: number; card: Promise<AtlasSceneCard> }>();

function cached<T>(cache: Map<number, { at: number; card: Promise<T> }>, id: number, load: () => Promise<T>): Promise<T> {
	const hit = cache.get(id);
	const now = Date.now();
	if (hit && now - hit.at < ARTIST_TTL_MS) return hit.card;
	const card = load();
	cache.set(id, { at: now, card });
	// A failure is not worth remembering: the next video asks again.
	card.catch(() => cache.delete(id));
	if (cache.size > 200) cache.delete(cache.keys().next().value as number);
	return card;
}

export function clearStageCache() {
	artistCache.clear();
	sceneCache.clear();
}

/** Build every row for one video, calling `onRows` as each lands so the
 *  page fills in progressively. `isCurrent` lets the caller drop a stale
 *  video's rows. Never throws. */
export async function loadStage(
	client: StageClient,
	video: StageVideo,
	onRows: (patch: Partial<StageRows>) => void,
	isCurrent: () => boolean = () => true
): Promise<void> {
	const emit = (patch: Partial<StageRows>) => {
		if (isCurrent()) onRows(patch);
	};

	// One song lookup feeds two rows: the versions/concert rows, and the
	// artist for the exploring rows when a cold link carried only the video id.
	const songLookup = findSong(client, video).catch(() => null);

	const songTask = (async () => {
		const song = await songLookup;
		if (!song) return;
		const ordered = orderVersions(song.versions);
		const others = ordered.filter((version) => version.video.tidal_id !== video.tidal_id);
		if (others.length > 0) emit({ versions: { song, others } });
		const mine = song.versions.find((version) => version.video.tidal_id === video.tidal_id);
		if (mine?.concert) {
			try {
				const concert = await client.getVideoConcert(mine.concert.id);
				if (concert.videos.length > 1) emit({ concert });
			} catch {
				// No concert row; the others stand.
			}
		}
	})().catch(() => {});

	const atlasTask = (async () => {
		const artistId = video.artist_id ?? (await songLookup)?.artist_id ?? null;
		if (artistId == null) return;
		let card: AtlasArtistCard;
		try {
			card = await cached(artistCache, artistId, () => client.getVideoAtlasArtist(artistId));
		} catch {
			return;
		}
		const sceneTask = (async () => {
			if (card.scene_id == null) return;
			const sceneId = card.scene_id;
			try {
				const sceneCard = await cached(sceneCache, sceneId, () => client.getVideoAtlasScene(sceneId));
				const videos = sceneCard.sample
					.filter((item) => item.tidal_id !== video.tidal_id)
					.slice(0, SCENE_SAMPLE_LIMIT);
				if (videos.length > 0) emit({ scene: { scene: sceneCard.scene, videos } });
			} catch {
				// Scene row stays empty.
			}
		})();
		const stepTask = (async () => {
			const neighbors = stepOutNeighbors(card);
			const picks = await Promise.all(neighbors.map(async (neighbor) => {
				try {
					const other = await cached(artistCache, neighbor.id, () => client.getVideoAtlasArtist(neighbor.id));
					const best = other.best.find((item) => item.tidal_id !== video.tidal_id);
					return best ? { video: best, neighbor } : null;
				} catch {
					return null;
				}
			}));
			const stepOut = picks.filter((pick): pick is StepOutPick => pick !== null);
			if (stepOut.length > 0) emit({ stepOut });
		})();
		await Promise.all([sceneTask, stepTask]);
	})().catch(() => {});

	await Promise.all([songTask, atlasTask]);
}

/** The picks that open the watch page's related grid: the same scene and
 *  one step out, woven two to one so the first rows mix the familiar with
 *  the new, each saying why it is there. The playing video and repeats are
 *  left out; related and radio picks follow them in the same grid. */
export function explorePicks(rows: StageRows, playingId: number): ExploreVideo[] {
	const near = (rows.scene?.videos ?? []).map((video) => ({ ...video, why: video.why || 'Same scene' }));
	const out = rows.stepOut.map(({ video, neighbor }) => ({ ...video, why: `One step out, ${viaLabel(neighbor.source)}` }));
	const woven: ExploreVideo[] = [];
	let n = 0;
	let o = 0;
	while (n < near.length || o < out.length) {
		if (n < near.length) woven.push(near[n++]);
		if (n < near.length) woven.push(near[n++]);
		if (o < out.length) woven.push(out[o++]);
	}
	const seen = new Set<number>([playingId]);
	return woven.filter((video) => {
		if (seen.has(video.tidal_id)) return false;
		seen.add(video.tidal_id);
		return true;
	});
}
