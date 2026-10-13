// The Stage: the rows under the watch page's player. Each row comes from the
// Atlas and Air endpoints; any of them may answer 501 (not built yet) or fail,
// and the page falls back to the old related grid when the exploring rows
// (same scene, one step out) come back empty.

import type {
	AtlasArtistCard,
	AtlasNeighbor,
	AtlasScene,
	AtlasSceneCard,
	ArchiveQuery,
	ArchiveResponse,
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
	getVideoSongVersions(songKey: string): Promise<SongVersionsResponse>;
	getVideoConcert(id: number): Promise<VideoConcert>;
	getVideoAtlasArtist(id: number): Promise<AtlasArtistCard>;
	getVideoAtlasScene(id: number): Promise<AtlasSceneCard>;
	getVideoArchive(query: ArchiveQuery, signal?: AbortSignal): Promise<ArchiveResponse>;
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

/** The server's song key (video_radio::video_song_key): artist id (or
 *  lowercased name), then the title before any bracket, lowercased, every
 *  non letter or digit a space, whitespace collapsed. Mirrored here so the
 *  Stage can ask for a song's versions without a lookup round trip. */
export function songKeyFor(video: Pick<StageVideo, 'title' | 'artist_id' | 'artist_name'>): string {
	const artist = video.artist_id != null
		? `id:${video.artist_id}`
		: `name:${(video.artist_name ?? '').trim().toLowerCase()}`;
	const normalize = (text: string) =>
		text
			.toLowerCase()
			.replace(/[^\p{L}\p{N}]+/gu, ' ')
			.trim()
			.split(/\s+/)
			.filter(Boolean)
			.join(' ');
	const base = video.title.split(/[([]/)[0] ?? video.title;
	const title = normalize(base) || normalize(video.title);
	return `${artist}:${title}`;
}

/** The title without its bracketed suffix, for the archive search fallback. */
export function baseTitle(title: string): string {
	const base = (title.split(/[([]/)[0] ?? title).trim();
	return base || title.trim();
}

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

/** The playing video's song: by the mirrored key first; when that misses
 *  (key drift, an unknown title), an archive search on the bare title
 *  narrowed to the same artist. Null when neither finds it. */
export async function findSong(client: StageClient, video: StageVideo): Promise<SongVersionsResponse | null> {
	try {
		const song = await client.getVideoSongVersions(songKeyFor(video));
		if (hasVideo(song, video.tidal_id)) return song;
	} catch (error) {
		if (status(error) === 501) return null;
	}
	try {
		const result = await client.getVideoArchive({ q: baseTitle(video.title), limit: 24 });
		const match = result.items.find((item) =>
			item.best.tidal_id === video.tidal_id
			|| (video.artist_id != null && item.artist_id === video.artist_id && songKeyFor({ ...video, title: item.title }) === songKeyFor(video))
		) ?? result.items.find((item) => video.artist_id != null && item.artist_id === video.artist_id);
		if (!match) return null;
		const song = await client.getVideoSongVersions(match.song_key);
		return hasVideo(song, video.tidal_id) || match.best.tidal_id === video.tidal_id ? song : null;
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

	const songTask = (async () => {
		const song = await findSong(client, video);
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
		if (video.artist_id == null) return;
		const artistId = video.artist_id;
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

/** The exploring rows came back empty: show the old related grid instead. */
export function needsFallback(rows: StageRows): boolean {
	return rows.scene === null && rows.stepOut.length === 0;
}
