// Atlas and Air: the wire contract for the video exploration endpoints.
// The Rust handlers (video_intel_routes, video_atlas_routes,
// video_channel_routes) serialize exactly these shapes. Change both sides
// together.

import type { TidalSearchVideo } from './types';

/** A catalog video as every new endpoint sends it (VideoSetItem on the
 *  server). `why` is a short reason chip, empty when nothing fired. `cut`
 *  and `song_year` ride along when the catalog intelligence knows them
 *  (`song_year` only when its era is confident). */
export type ExploreVideo = TidalSearchVideo & {
	why?: string;
	cut?: VideoCut;
	song_year?: number | null;
};

/** The kind of a video, from multilingual title markers, TIDAL type and
 *  duration. `clip` is anything under 90 s; `longform` is 30 min and up. */
export type VideoCut =
	| 'official'
	| 'live'
	| 'session'
	| 'lyric'
	| 'visualizer'
	| 'remix'
	| 'dance'
	| 'interview'
	| 'trailer'
	| 'clip'
	| 'longform';

/** Light is knowledge: 0 uncharted (no graph edge), 1 dim (charted, never
 *  touched), 2 warm (library artist, or one hop from lit or library), 3 lit
 *  (watched, liked or saved). */
export type Light = 0 | 1 | 2 | 3;

// ---------------------------------------------------------------- Atlas

export interface AtlasScene {
	id: number;
	/** Pinned name, else exemplar name ("Coldplay, Arcade Fire"). */
	name: string;
	exemplars: string[];
	tags: string[];
	/** Degrees 0..360: the scene's angle around the map centre. Color is place. */
	hue: number;
	/** Map units: the map is a 1000 x 1000 square, origin top-left. */
	x: number;
	y: number;
	r: number;
	videos: number;
	artists: number;
	hours: number;
	/** Stable channel number once the scene qualifies for On Air, else null. */
	channel_no: number | null;
	/** Artists in the scene the listener has lit (watched, liked, saved). */
	lit_artists: number;
	/** Watched-video count in the scene, for "seen N of M". */
	seen_videos: number;
}

/** Columnar so 15k-60k artists stay small: index i across every array is
 *  one artist. scene = -1 means uncharted. */
export interface AtlasArtists {
	ids: number[];
	x: number[];
	y: number[];
	scene: number[];
	videos: number[];
	light: Light[];
}

export interface AtlasResponse {
	built_at: string | null;
	/** True while the background build has not produced a map yet. */
	building: boolean;
	size: number; // always 1000
	scenes: AtlasScene[];
	artists: AtlasArtists;
	/** Artists the crawler touched in the last hour, for faint pulses. */
	recent_artist_ids: number[];
}

export interface AtlasNamesResponse {
	ids: number[];
	names: string[];
}

export interface AtlasNeighbor {
	id: number;
	name: string;
	/** tidal | lastfm | featured | colist */
	source: string;
	weight: number;
	light: Light;
}

export interface AtlasArtistCard {
	id: number;
	name: string;
	scene_id: number | null;
	scene_name: string | null;
	videos: number;
	playable: number;
	hours: number;
	light: Light;
	charted: boolean;
	genres: string[];
	/** Best cuts first, up to 8. */
	best: ExploreVideo[];
	neighbors: AtlasNeighbor[];
}

export interface AtlasSceneCard {
	scene: AtlasScene;
	top_artists: { id: number; name: string; videos: number; light: Light }[];
	/** A short programmable sample, best cuts, spaced by artist. */
	sample: ExploreVideo[];
}

export interface AtlasProbeResponse {
	queued: boolean;
	/** Why not, when queued is false: "discovery off", "already checked". */
	reason?: string;
}

export interface AtlasRouteStop {
	artist_id: number;
	name: string;
	via: string | null; // edge source that led here
	video: ExploreVideo | null;
}

export interface AtlasRouteResponse {
	found: boolean;
	stops: AtlasRouteStop[];
}

// ---------------------------------------------------------------- Catalog

/** One scene's coverage for the Logbook: "seen N of M" without the atlas. */
export interface CensusScene {
	id: number;
	name: string;
	hue: number;
	videos: number;
	artists: number;
	/** Distinct watched videos by the scene's artists. */
	seen_videos: number;
	/** Scene artists the listener watched or saved. */
	lit_artists: number;
	channel_no: number | null;
}

export interface VideoCensus {
	building: boolean;
	built_at: string | null;
	videos: number;
	hours: number;
	programmable: number;
	artists: number;
	charted_artists: number;
	uncharted_artists: number;
	scenes: number;
	channel_scenes: number;
	songs: number;
	multi_cut_songs: number;
	concerts: number;
	cuts: Record<VideoCut, number>;
	genre_coverage: number; // 0..1 of charted artists
	/** 0..1 of songs whose year has era confidence >= 0.5 (library, tag, or
	 *  a cut year the artist's range backs); a bare upload year does not count. */
	era_coverage: number;
	watched_videos: number;
	lit_artists: number;
	stages: { stage: string; finished_at: string | null; rows: number | null; error: string | null }[];
	/** Per-scene coverage, biggest first; empty before scenes exist. */
	scene_rows: CensusScene[];
}

export type ArchiveSort = 'relevance' | 'newest' | 'year' | 'popular' | 'versions';
export type ArchiveDepth = 'big' | 'mid' | 'deep';
export type ArchiveLength = 'clip' | 'song' | 'long';
export type ArchiveSource = 'catalog' | 'liked' | 'saved';

export interface ArchiveQuery {
	scene?: number;
	/** Only this artist's songs (TIDAL id). Scopes every facet count. */
	artist?: number;
	cut?: VideoCut;
	era_from?: number;
	era_to?: number;
	depth?: ArchiveDepth;
	length?: ArchiveLength;
	source?: ArchiveSource;
	sort?: ArchiveSort;
	q?: string;
	offset?: number;
	limit?: number;
}

export interface ArchiveSong {
	song_key: string;
	title: string;
	artist_id: number | null;
	artist_name: string | null;
	best: ExploreVideo;
	cut_count: number;
	cuts: VideoCut[];
	song_year: number | null;
	scene_id: number | null;
	light: Light;
}

export interface ArchiveResponse {
	/** True until the first catalog build has finished. */
	building: boolean;
	total: number;
	items: ArchiveSong[];
	/** Live counts for every lens under the other active lenses. */
	facets: {
		scene: { id: number; name: string; hue: number; count: number }[];
		cut: { cut: VideoCut; count: number }[];
		decade: { decade: number; count: number }[];
		depth: { depth: ArchiveDepth; count: number }[];
		length: { length: ArchiveLength; count: number }[];
	};
}

export interface VideoVersion {
	video: ExploreVideo;
	cut: VideoCut;
	cut_year: number | null;
	/** What tells this cut apart: "Live at Croke Park", "Concert film, 2016". */
	label: string;
	concert: { id: number; label: string } | null;
	best: boolean;
}

export interface SongVersionsResponse {
	song_key: string;
	title: string;
	artist_id: number | null;
	artist_name: string | null;
	song_year: number | null;
	versions: VideoVersion[];
}

export interface VideoConcert {
	id: number;
	artist_id: number;
	artist_name: string | null;
	year: number | null;
	label: string;
	video_count: number;
	total_s: number;
	videos: ExploreVideo[];
}

// ---------------------------------------------------------------- On Air

export type ExploreMode = 'familiar' | 'mixed' | 'adventurous';

export interface ChannelProgram {
	slot: number;
	/** Seconds since local midnight. */
	start_s: number;
	duration_s: number;
	video: ExploreVideo;
	why: string;
}

export interface VideoChannel {
	/** your | wild | premieres | live | sessions | concerts | rewind |
	 *  spotlight | scene:{id} */
	id: string;
	/** Stable: fixed channels 1-8, scene channels keep theirs forever.
	 *  0 = not numbered yet: a locked "coming up" scene listed after the
	 *  numbered channels. */
	no: number;
	title: string;
	subtitle: string;
	/** Scene hue for scene channels; null for the fixed channels. */
	hue: number | null;
	scene_id: number | null;
	locked: boolean;
	/** For locked channels: hours playable now and hours needed. */
	unlock: { hours: number; need_hours: number } | null;
	now: (ChannelProgram & { offset_s: number }) | null;
	next: ChannelProgram | null;
	/** Scene appetite (about 0.05-1.1) for ordering a "for you" strip; null
	 *  on the fixed channels. Never changes the numbers. */
	appetite?: number | null;
}

export interface VideoChannelsResponse {
	day: string;
	/** Server's seconds since local midnight when the response was built. */
	now_s: number;
	explore: ExploreMode;
	building: boolean;
	channels: VideoChannel[];
}

export interface ChannelGuideResponse {
	channel_id: string;
	day: string;
	programs: ChannelProgram[];
}

export interface ChannelTuneResponse {
	channel: VideoChannel;
	/** The scheduled video and where in it to start. */
	video: ExploreVideo;
	offset_s: number;
	/** What follows on the schedule (12 items). */
	queue: ExploreVideo[];
}

export interface ChannelNextResponse {
	items: ExploreVideo[];
	exhausted: boolean;
}

/** More / Less like this on a channel. For More, `items` is what to line
 *  up right after the playing video (a neighbour, the artist, another
 *  neighbour); Less lines up nothing. */
export interface ChannelSteerResponse {
	items: ExploreVideo[];
	remembered: boolean;
}

/** A remembered More / Less press: a small lean on the artist in On Air
 *  programming that halves every week and is gone after four. */
export interface VideoSteer {
	artist_id: number;
	name: string | null;
	more: boolean;
	/** Unix seconds of the press. */
	at: number;
	/** 1 at the press, fading to 0. */
	strength: number;
	/** Unix seconds when it is gone. */
	forgotten_at: number;
}
