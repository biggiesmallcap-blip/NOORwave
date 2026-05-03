// All TypeScript types for the DiscoverSpace visualization.

export type DiscoverSource = 'library' | 'lastfm' | 'engine' | 'mixed';

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

export interface DiscoverTrackNode {
	// Identity
	id: string;                        // "track-${track_id}"
	trackId: number;
	title: string;
	artist: string;
	albumTitle?: string;
	artworkUrl?: string;
	durationMs?: number;

	// Source
	source: DiscoverSource;
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
	confidence: number;                // 0..1
	supportCount: number;
	inDegree: number;
	inDegreePctile: number;            // 0..1 within current set

	// Reasoning
	primaryReason: DiscoverReason;
	reasonTags: DiscoverReason[];

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
	confidence?: number;
	support_count?: number;
	candidate_in_degree?: number;
	candidate_in_degree_percentile?: number;
	primary_reason?: string;
	reason_tags?: string[];
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
	};
}
