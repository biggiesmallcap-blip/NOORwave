import type { PlayableTrack } from '$lib/player/playable';

// All TypeScript types for the DiscoverSpace visualization.

export type DiscoverSource = 'library' | 'lastfm' | 'engine' | 'external' | 'mixed';

export type DiscoverReason =
	| 'harmonic'
	| 'behavioral'
	| 'bpm'
	| 'artist'
	| 'album'
	| 'genre'
	| 'energy'
	| 'external'
	| 'unknown';

export type DiscoverLens = 'energy' | 'reason' | 'confidence' | 'source' | 'genre';

export type RadioMode = 'radio' | 'explore' | 'harmonic' | 'energy_arc';

export type DiscoverRole = 'seed' | 'external_candidate' | 'library_guide' | 'route';
export type DiscoverPlayability = 'playable' | 'resolvable' | 'pending' | 'unavailable';
export type DiscoverBlendSeedKind = 'library' | 'tidal' | 'pending';

export interface DiscoverBlendSeed {
	kind: DiscoverBlendSeedKind;
	identity: string;
	track_id?: number | null;
	tidal_id?: number | null;
	artist?: string | null;
	title?: string | null;
	weight?: number | null;
}

export interface DiscoverBlendHealth {
	playable_external_count: number;
	pending_external_count: number;
	library_guide_count: number;
	coverage_ratio: number;
}

export interface DiscoverBlendSeedScore {
	seed_identity: string;
	seed_track_id?: number | null;
	score: number;
}

export interface DiscoverTrackNode {
	// Identity
	id: string;                        // "track-${track_id}"
	trackId: number;
	title: string;
	artist: string;
	albumTitle?: string;
	artworkUrl?: string;
	durationMs?: number;
	playable: PlayableTrack;

	// Source
	source: DiscoverSource;
	role: DiscoverRole;
	playability: DiscoverPlayability;
	isInLibrary: boolean;
	isColdStart: boolean;

	// Genres
	topGenre?: string;
	genres: string[];

	// Audio features
	energy?: number;                   // 0..1
	danceability?: number;             // 0..1
	bpm?: number;
	camelotKey?: string;

	// Relevance
	score: number;                     // 0..1, normalized per source
	rawScore?: number;
	shapedScore?: number;              // pre-normalization multi-signal score
	rerankScore?: number;              // session-taste rerank result, list-sort only
	confidence: number;                // 0..1
	supportCount: number;
	inDegree: number;
	inDegreePctile: number;            // 0..1 within current set

	// Reasoning
	why?: string;                      // compact human-readable relation summary
	whySignals?: string[];             // stable keys behind `why` (key_bpm, genre, ...)
	primaryReason: DiscoverReason;
	reasonTags: DiscoverReason[];
	perSeedScores: DiscoverBlendSeedScore[];
	coverageBonus: number;
	externalBonus: number;
	libraryPenalty: number;
	finalBlendScore?: number;

	// State flags (client-derived)
	isSeed: boolean;
	isPlaying: boolean;
	inPlaylistBuilder: boolean;
	isRouteOnly: boolean;              // ghost node for radio route

	// Layout (server hint or client-computed)
	x: number;
	y: number;
	vx: number;
	vy: number;
	radius: number;

	// v1.5 layout hint from server
	layoutHint?: {
		x?: number;
		y?: number;
		radiusHint?: number;
		clusterKey?: string;
		distanceFromSeed?: number;
	};
}

export interface DiscoverEdge {
	id: string;
	fromTrackId: number;
	toTrackId: number;
	reason: DiscoverReason;
	primaryReason: DiscoverReason;
	reasonTags: DiscoverReason[];
	weight: number;                    // 0..1 for thickness
	confidence: number;                // 0..1 for alpha
	source: DiscoverSource;
	supportCount?: number;
}

export interface DiscoverRouteStep {
	trackId: number;
	reason: DiscoverReason;
	stepIndex: number;
	isCurrent: boolean;
}

export interface VisitedRegion {
	label: string;
	centroid: { x: number; y: number; radius: number };
}

export interface Camera {
	x: number;
	y: number;
	zoom: number;
}

export interface TrainingState {
	isRunning: boolean;
	phase: string;
	tracksTotal: number;
	tracksDone: number;
	progress: number;
}

// Raw API response shapes (before adapter maps them)
export interface ApiDiscoveryNode {
	id?: string;
	track_id: number;
	title: string;
	artist_name: string;
	album_title?: string;
	artwork_url?: string;
	duration_ms?: number;
	source?: string;
	role?: DiscoverRole;
	playability?: DiscoverPlayability;
	is_in_library: boolean;
	is_cold_start?: boolean;
	top_genre?: string;
	genres?: string[];
	energy?: number;
	danceability?: number;
	bpm?: number;
	camelot_key?: string;
	score?: number;
	raw_score?: number;
	similarity_score?: number;
	shaped_score?: number;
	why?: string;
	why_signals?: string[];
	confidence?: number;
	support_count?: number;
	candidate_in_degree?: number;
	candidate_in_degree_percentile?: number;
	primary_reason?: string;
	reason_tags?: string[];
	per_seed_scores?: DiscoverBlendSeedScore[];
	coverage_bonus?: number;
	external_bonus?: number;
	library_penalty?: number;
	final_blend_score?: number;
	is_seed?: boolean;
	layout?: {
		x?: number;
		y?: number;
		radius_hint?: number;
		cluster_key?: string;
		distance_from_seed?: number;
	};
}

export interface ApiDiscoveryEdge {
	id?: string;
	from_id?: number;
	to_id?: number;
	from_track_id?: number;
	to_track_id?: number;
	reason?: string;
	primary_reason?: string;
	reason_tags?: string[];
	type?: string;
	weight: number;
	confidence?: number;
	source?: string;
	support_count?: number;
}

export interface ApiDiscoveryResponse {
	tracks: ApiDiscoveryNode[];
	edges: ApiDiscoveryEdge[];
	seed_track_id?: number;
	blend_seeds?: DiscoverBlendSeed[];
	health?: DiscoverBlendHealth;
	generated_at?: string;
	diagnostics?: {
		node_count: number;
		edge_count: number;
		source_counts: Record<string, number>;
		reason_counts: Record<string, number>;
		avg_confidence: number;
		avg_in_degree_percentile?: number;
		raw_candidate_count: number;
		raw_edge_count: number;
		pruned_node_count: number;
		pruned_edge_count: number;
		hub_suppressed_count: number;
		low_confidence_edge_dropped_count: number;
		coherence?: number;
		filter_dropped_count?: number;
		era_filter_coverage?: number | null;
		rerank_applied?: boolean;
	};
}

/// One hop in the branch history: the seed the user was on before branching.
export interface BranchStep {
	seedTrackId: number;
	title: string;
	artist: string;
}

/// User-set discovery filters, mirroring the backend SpaceFilters request
/// shape. All fields optional; an all-default object sends no constraints.
export interface DiscoverFilters {
	bpm_min?: number | null;
	bpm_max?: number | null;
	energy_min?: number | null;
	energy_max?: number | null;
	key_compatible_only?: boolean;
	year_min?: number | null;
	year_max?: number | null;
	exclude_in_library?: boolean;
	exclude_heard_session?: boolean;
}

export function isFilterNoop(filters: DiscoverFilters): boolean {
	return (
		filters.bpm_min == null &&
		filters.bpm_max == null &&
		filters.energy_min == null &&
		filters.energy_max == null &&
		!filters.key_compatible_only &&
		filters.year_min == null &&
		filters.year_max == null &&
		!filters.exclude_in_library &&
		!filters.exclude_heard_session
	);
}
