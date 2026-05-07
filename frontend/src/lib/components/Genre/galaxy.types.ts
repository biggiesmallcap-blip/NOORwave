// Desaturated to ~65% of original hue, blended 35% toward a shared #c6c8e2
// periwinkle anchor. Preserves family identity while dropping the "rainbow"
// read the user flagged in the design-refresh chat.
export const ROOT_FAMILY_COLORS = {
	electronic: { label: 'Electronic', color: '#9699f5', glowColor: 'rgba(150, 153, 245, 0.22)' },
	rock: { label: 'Rock', color: '#eb8bb5', glowColor: 'rgba(235, 139, 181, 0.20)' },
	pop: { label: 'Pop', color: '#ebce91', glowColor: 'rgba(235, 206, 145, 0.20)' },
	'hip-hop': { label: 'Hip-Hop', color: '#e0be8a', glowColor: 'rgba(224, 190, 138, 0.20)' },
	'r-b-and-soul': { label: 'R&B and Soul', color: '#c697f5', glowColor: 'rgba(198, 151, 245, 0.22)' },
	jazz: { label: 'Jazz', color: '#49d1b7', glowColor: 'rgba(73, 209, 183, 0.20)' },
	classical: { label: 'Classical', color: '#dcc594', glowColor: 'rgba(220, 197, 148, 0.20)' },
	'folk-and-country': { label: 'Folk and Country', color: '#a3c196', glowColor: 'rgba(163, 193, 150, 0.20)' },
	latin: { label: 'Latin', color: '#e65da5', glowColor: 'rgba(230, 93, 165, 0.22)' },
	'reggae-and-caribbean': { label: 'Reggae and Caribbean', color: '#71b4a9', glowColor: 'rgba(113, 180, 169, 0.20)' },
	blues: { label: 'Blues', color: '#76c8eb', glowColor: 'rgba(118, 200, 235, 0.20)' },
	'ambient-and-experimental': {
		label: 'Ambient and Experimental',
		color: '#b2e0de',
		glowColor: 'rgba(178, 224, 222, 0.18)'
	},
	'soundtrack-and-screen': {
		label: 'Soundtrack and Screen',
		color: '#e3ae8e',
		glowColor: 'rgba(227, 174, 142, 0.20)'
	},
	world: { label: 'World', color: '#db8e83', glowColor: 'rgba(219, 141, 131, 0.20)' }
} as const;

export type RootFamilyKey = keyof typeof ROOT_FAMILY_COLORS;

export interface GalaxyNode {
	id: number;
	name: string;
	slug: string;
	parentId: number | null;
	familyId: number;
	familyKey: RootFamilyKey;
	familyName: string;
	depth: number;
	trackCount: number;
	listenCount: number;
	totalListenedMs: number;
	x: number;
	y: number;
	vx: number;
	vy: number;
	radius: number;
	heatNorm: number;
	color: string;
	glowColor: string;
	cohortId: string | null;
	evolutionHistory: { periodStart: string; listenCount: number }[];
	avgBpm: number | null;
	avgEnergy: number | null;
	avgDanceability: number | null;
}

export interface GalaxyEdge {
	sourceId: number;
	targetId: number;
	type: 'parent-child' | 'sibling';
	weight: number;
}

export interface GalaxyCohort {
	id: string;
	label: string;
	icon: string;
	genreIds: number[];
	listenCount: number;
}

export interface GalaxyConfig {
	viewMode: 'map' | 'heat' | 'vibe' | 'rediscover';
	showCohorts: boolean;
	showEvolution: boolean;
}

export interface Camera {
	x: number;
	y: number;
	scale: number;
	targetX: number;
	targetY: number;
	targetScale: number;
}

export interface HeatParticle {
	edgeIndex: number;
	t: number;
	speed: number;
	alpha: number;
	size: number;
}

export interface GalaxyData {
	nodes: GalaxyNode[];
	edges: GalaxyEdge[];
}

export type ZoomLevel = 'galaxy' | 'cluster' | 'node';
export type GalaxyViewMode = 'map' | 'heat' | 'vibe' | 'rediscover';

export const GALAXY_ROOT_RING_RADIUS = 380;
export const GALAXY_DEFAULT_SCALE = 0.58;

export function familyKeyFromSlug(slug: string): RootFamilyKey {
	if (slug in ROOT_FAMILY_COLORS) {
		return slug as RootFamilyKey;
	}
	return 'world';
}
