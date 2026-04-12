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
}

export interface DiscoverySpaceResponse {
  tracks: DiscoverTrackNode[];
  artists: DiscoverArtistNode[];
  edges: DiscoverEdge[];
}
