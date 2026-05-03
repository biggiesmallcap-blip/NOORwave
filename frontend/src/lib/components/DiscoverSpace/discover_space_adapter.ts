// Thin mapper: API response → typed DiscoverTrackNode / DiscoverEdge.
// Defensive ?? defaults allow the frontend to run against a partially-deployed
// backend without crashing. Once v1.5 is fully deployed, the ?? branches are
// never exercised in production.

import type {
	DiscoverTrackNode,
	DiscoverEdge,
	DiscoverReason,
	DiscoverSource,
	ApiDiscoveryNode,
	ApiDiscoveryEdge,
} from './discover_space_types';

// ─── Deterministic initial layout ────────────────────────────────────────────

function hash32(s: string): number {
	let h = 2166136261;
	for (let i = 0; i < s.length; i++) {
		h = Math.imul(h ^ s.charCodeAt(i), 16777619) >>> 0;
	}
	return h;
}

export function deterministicInitialPosition(
	trackId: number,
	seedId: number,
	radius = 300
): { x: number; y: number } {
	const h = hash32(`${trackId}:${seedId}`);
	const angle = ((h % 10000) / 10000) * Math.PI * 2;
	const dist = radius * (0.5 + ((h >>> 16) % 1000) / 1000 * 0.5);
	return { x: Math.cos(angle) * dist, y: Math.sin(angle) * dist };
}

// ─── Reason normalization (mirrors Rust normalizer) ──────────────────────────

const REASON_MAP: Record<string, DiscoverReason> = {
	harmonic: 'harmonic', harmonic_match: 'harmonic', audio_texture: 'harmonic',
	behavioural: 'behavioral', behavioral: 'behavioral', same_pocket: 'behavioral', taste_mesh: 'behavioral',
	bpm_match: 'bpm',
	artist_affinity: 'artist', artist_seed: 'artist', artist_repeat: 'artist', artist_continuity: 'artist',
	album_context: 'album', album_seed: 'album', connected_album_seed: 'album',
	genre_branch: 'genre', genre_affinity: 'genre', genre_drift: 'genre', prompt_genre: 'genre',
	energy_match: 'energy',
	external_match: 'external', 'last.fm similar': 'external', discogs_style: 'external',
	prompt_match: 'external', scene_match: 'external',
};

export function normalizeReason(tag?: string): DiscoverReason {
	if (!tag) return 'unknown';
	return REASON_MAP[tag.trim()] ?? 'unknown';
}

export function normalizeReasonTags(tags?: string[]): DiscoverReason[] {
	if (!tags || tags.length === 0) return [];
	const seen = new Set<string>();
	return tags.map(normalizeReason).filter((r) => {
		if (seen.has(r)) return false;
		seen.add(r);
		return true;
	});
}

function clamp01(v: number): number {
	return Math.max(0, Math.min(1, v));
}

function normalizeSource(raw?: string): DiscoverSource {
	switch (raw) {
		case 'library':
		case 'tidal':
			return 'library';
		case 'lastfm':
		case 'last.fm':
			return 'lastfm';
		case 'engine':
			return 'engine';
		case 'mixed':
			return 'mixed';
		default:
			return 'engine';
	}
}

// ─── Node adapter ─────────────────────────────────────────────────────────────

export function adaptNode(
	api: ApiDiscoveryNode,
	currentTrackId: number | null,
	seedId: number | null
): DiscoverTrackNode {
	const primaryReason = normalizeReason(
		api.primary_reason ?? api.reason_tags?.[0]
	);
	const reasonTags = normalizeReasonTags(
		api.reason_tags ?? (api.primary_reason ? [api.primary_reason] : [])
	);
	const score = api.score ?? clamp01(api.similarity_score ?? 0);
	const conf = api.confidence ?? (api.is_in_library ? 1.0 : 0.5);
	const isColdStart = api.is_cold_start ?? !api.is_in_library;
	const source = normalizeSource(api.source);
	const genres = api.genres ?? (api.top_genre ? [api.top_genre] : []);
	const supportCount = api.support_count ?? 0;
	const inDegree = api.candidate_in_degree ?? 0;
	const inDegreePctile = api.candidate_in_degree_percentile ?? 0;

	const isSeedNode = api.is_seed ?? api.track_id === seedId;

	// Seed lives at the world origin; orbit stars use score-based initial radius
	// so high-confidence matches start close and cold/distant ones orbit further out.
	let initX = 0, initY = 0;
	if (!isSeedNode) {
		const hintX = api.layout?.x;
		const hintY = api.layout?.y;
		if (hintX != null && hintY != null) {
			initX = hintX;
			initY = hintY;
		} else {
			const baseRadius = 150 + (1 - score) * 250 + (isColdStart ? 120 : 0);
			const pos = deterministicInitialPosition(api.track_id, seedId ?? 0, baseRadius);
			initX = pos.x;
			initY = pos.y;
		}
	}

	return {
		id: api.id ?? `track-${api.track_id}`,
		trackId: api.track_id,
		title: api.title,
		artist: api.artist_name,
		albumTitle: api.album_title,
		artworkUrl: api.artwork_url,
		durationMs: api.duration_ms,
		source,
		isInLibrary: api.is_in_library,
		isColdStart,
		topGenre: api.top_genre,
		genres,
		energy: api.energy,
		danceability: api.danceability,
		bpm: api.bpm,
		camelotKey: api.camelot_key,
		score,
		rawScore: api.raw_score ?? api.similarity_score,
		confidence: conf,
		supportCount,
		inDegree,
		inDegreePctile,
		primaryReason,
		reasonTags,
		isSeed: isSeedNode,
		isPlaying: api.track_id === currentTrackId,
		inPlaylistBuilder: false,
		isRouteOnly: false,
		x: initX,
		y: initY,
		vx: 0,
		vy: 0,
		radius: api.layout?.radius_hint ?? (isSeedNode ? 24 : 5 + score * 18),
		layoutHint: api.layout,
	};
}

// ─── Edge adapter ─────────────────────────────────────────────────────────────

export function adaptEdge(api: ApiDiscoveryEdge): DiscoverEdge {
	const fromId = api.from_track_id ?? api.from_id ?? 0;
	const toId = api.to_track_id ?? api.to_id ?? 0;
	const primaryReason = normalizeReason(
		api.primary_reason ?? api.reason ?? api.type ?? api.reason_tags?.[0]
	);
	const reasonTags = normalizeReasonTags(
		api.reason_tags ?? (api.reason ? [api.reason] : [])
	);
	const source = normalizeSource(api.source);
	const edgeId = api.id ?? `${fromId}-${toId}-${primaryReason}`;

	return {
		id: edgeId,
		fromTrackId: fromId,
		toTrackId: toId,
		reason: primaryReason,
		primaryReason,
		reasonTags,
		weight: clamp01(api.weight),
		confidence: api.confidence ?? 0.5,
		source,
		supportCount: api.support_count,
	};
}

// ─── Response adapter ─────────────────────────────────────────────────────────

export function adaptResponse(
	data: { tracks?: ApiDiscoveryNode[]; edges?: ApiDiscoveryEdge[] },
	currentTrackId: number | null,
	seedId: number | null
): { nodes: DiscoverTrackNode[]; edges: DiscoverEdge[] } {
	const nodes = (data.tracks ?? []).map((n) => adaptNode(n, currentTrackId, seedId));
	const edges = (data.edges ?? []).map(adaptEdge);
	return { nodes, edges };
}

// ─── Ghost node factory (for radio route candidates not in space graph) ────────

export function makeGhostNode(
	trackId: number,
	title: string,
	artist: string,
	reason: DiscoverReason,
	prevNode: DiscoverTrackNode,
	seedId: number | null
): DiscoverTrackNode {
	const angle = Math.random() * Math.PI * 2;
	const drift = 80 + Math.random() * 60;
	return {
		id: `track-${trackId}`,
		trackId,
		title,
		artist,
		source: 'engine',
		isInLibrary: false,
		isColdStart: true,
		genres: [],
		score: 0.3,
		confidence: 0.3,
		supportCount: 0,
		inDegree: 0,
		inDegreePctile: 0,
		primaryReason: reason,
		reasonTags: [reason],
		isSeed: false,
		isPlaying: false,
		inPlaylistBuilder: false,
		isRouteOnly: true,
		x: prevNode.x + Math.cos(angle) * drift,
		y: prevNode.y + Math.sin(angle) * drift,
		vx: 0,
		vy: 0,
		radius: 6,
	};
}
