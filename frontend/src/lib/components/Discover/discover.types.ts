export type DiscoverViewMode = 'radio' | 'explore' | 'harmonic' | 'energy_arc' | 'samples';

export interface DiscoverTrackNode {
  track_id: number;
  title: string;
  artist_name: string;
  album_title: string | null;
  artwork_url: string | null;
  duration_ms: number | null;
  similarity_score: number;       // 0-1
  energy: number | null;          // 0-1
  danceability: number | null;    // 0-1
  bpm: number | null;
  key_signature: string | null;
  camelot_key: string | null;
  is_in_library: boolean;
  source: 'tidal' | 'external';
  x: number;
  y: number;
  vx: number;
  vy: number;
  radius: number;
  opacity: number;
  // New in Phase 1 — optional, render later in Phase 2
  is_instrumental?: boolean | null;
  loudness_lufs?: number | null;
  skip_rate?: number | null;          // 0-1, 1 = always skipped
  completion_avg?: number | null;     // 0-1, average fraction listened
  cohort_id?: string | null;          // e.g. "night_owl"
  cohort_label?: string | null;       // e.g. "Night Owl"
  top_genre?: string | null;
  top_genre_source?: string | null;   // 'tidal' | 'spotify' | 'musicbrainz' | 'lastfm'
  top_genre_confidence?: number | null;
  last_played_at?: string | null;     // ISO date string
  play_count?: number;
}

export interface DiscoverArtistNode {
  artist_id: number;
  name: string;
  top_genre: string | null;
  affinity: number;               // 0-1
  x: number;
  y: number;
  vx: number;
  vy: number;
  size: number;
}

export interface DiscoverEdge {
  from_id: number;
  to_id: number;
  type: 'bpm_match' | 'harmonic' | 'behavioural' | 'sample' | 'genre';
  weight: number;                 // 0-1
  // New in Phase 1
  reason_tags?: string[];
  behavioral_score?: number;
  audio_score?: number;
  metadata_score?: number;
}

export interface DiscoverySpaceResponse {
  tracks: DiscoverTrackNode[];
  artists: DiscoverArtistNode[];
  edges: DiscoverEdge[];
}

// Phase 1 — meta endpoint
export interface DiscoverySpaceMeta {
  model_key: string | null;
  model_status: string | null;     // 'idle' | 'training' | 'ready' | 'active'
  trained_at: string | null;       // ISO date string
  vector_dim: number | null;
  neighbor_coverage: number;       // 0-1
  track_count_with_embeddings: number;
  track_count_total: number;
}
