import type { DiscoverTrackNode, DiscoverArtistNode, DiscoverEdge, DiscoverViewMode } from './discover.types';

const REPULSION = 800;
const ATTRACTION = 0.005;
const DAMPING = 0.85;
const CENTER_GRAVITY = 0.01;

export function applyForces(
  nodes: (DiscoverTrackNode | DiscoverArtistNode)[],
  edges: DiscoverEdge[],
  mode: DiscoverViewMode,
  dt: number = 1
) {
  // Repulsion between all nodes
  for (let i = 0; i < nodes.length; i++) {
    for (let j = i + 1; j < nodes.length; j++) {
      const dx = nodes[j]!.x - nodes[i]!.x;
      const dy = nodes[j]!.y - nodes[i]!.y;
      const dist = Math.sqrt(dx * dx + dy * dy) || 1;
      const force = REPULSION / (dist * dist);
      const fx = (dx / dist) * force;
      const fy = (dy / dist) * force;
      nodes[i]!.vx -= fx;
      nodes[i]!.vy -= fy;
      nodes[j]!.vx += fx;
      nodes[j]!.vy += fy;
    }
  }

  // Attraction along edges
  for (const edge of edges) {
    const from = nodes.find(n => ('track_id' in n && n.track_id === edge.from_id) || ('artist_id' in n && (n as DiscoverArtistNode).artist_id === edge.from_id));
    const to = nodes.find(n => ('track_id' in n && n.track_id === edge.to_id) || ('artist_id' in n && (n as DiscoverArtistNode).artist_id === edge.to_id));
    if (!from || !to) continue;
    const dx = to.x - from.x;
    const dy = to.y - from.y;
    const dist = Math.sqrt(dx * dx + dy * dy) || 1;
    const force = dist * ATTRACTION * edge.weight;
    const fx = (dx / dist) * force;
    const fy = (dy / dist) * force;
    from.vx += fx;
    from.vy += fy;
    to.vx -= fx;
    to.vy -= fy;
  }

  // Mode-specific forces
  if (mode === 'harmonic') {
    // Pull nodes toward their Camelot wheel position (simplified: cluster by key)
    const keyGroups = new Map<string, (DiscoverTrackNode | DiscoverArtistNode)[]>();
    for (const node of nodes) {
      const key = ('camelot_key' in node && node.camelot_key) || 'unknown';
      if (!keyGroups.has(key)) keyGroups.set(key, []);
      keyGroups.get(key)!.push(node);
    }
    for (const [key, group] of keyGroups) {
      if (key === 'unknown') continue;
      // Parse camelot key to get position on wheel (1-12 + A/B)
      const num = parseInt(key);
      if (isNaN(num)) continue;
      const isA = key.includes('A');
      const angle = ((num - 1) / 12) * Math.PI * 2 + (isA ? 0 : 0.26);
      const targetX = Math.cos(angle) * 300;
      const targetY = Math.sin(angle) * 300;
      for (const node of group) {
        node.vx += (targetX - node.x) * 0.02;
        node.vy += (targetY - node.y) * 0.02;
      }
    }
  } else if (mode === 'energy_arc') {
    // Pull low-energy left, high-energy right
    for (const node of nodes) {
      const energy = ('energy' in node ? node.energy : null) ?? 0.5;
      const targetX = (energy - 0.5) * 600;
      node.vx += (targetX - node.x) * 0.01;
    }
  }

  // Center gravity
  for (const node of nodes) {
    node.vx -= node.x * CENTER_GRAVITY;
    node.vy -= node.y * CENTER_GRAVITY;
  }

  // Integrate
  for (const node of nodes) {
    node.vx *= DAMPING;
    node.vy *= DAMPING;
    node.x += node.vx * dt;
    node.y += node.vy * dt;
  }
}

export function buildInitialLayout(
  tracks: any[],
  mode: DiscoverViewMode
): DiscoverTrackNode[] {
  return tracks.map((t, i) => {
    const angle = (i / Math.max(tracks.length, 1)) * Math.PI * 2;
    const radius = 100 + Math.random() * 200;
    return {
      track_id: t.track_id || t.id,
      title: t.title,
      artist_name: t.artist_name,
      album_title: t.album_title,
      artwork_url: t.artwork_url,
      duration_ms: t.duration_ms,
      similarity_score: t.similarity_score ?? 0.5,
      energy: t.energy,
      danceability: t.danceability,
      bpm: t.bpm,
      key_signature: t.key_signature,
      camelot_key: t.camelot_key,
      is_in_library: t.source === 'tidal',
      source: t.source || 'tidal',
      x: Math.cos(angle) * radius,
      y: Math.sin(angle) * radius,
      vx: 0,
      vy: 0,
      radius: 8 + (t.similarity_score ?? 0.5) * 24,
      opacity: 0,
    };
  });
}
