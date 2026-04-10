export const ROOT_FAMILY_COLORS = {
	electronic: { label: 'Electronic', color: '#7c80ff', glowColor: 'rgba(124, 128, 255, 0.34)' },
	rock: { label: 'Rock', color: '#ff6b9d', glowColor: 'rgba(255, 107, 157, 0.32)' },
	pop: { label: 'Pop', color: '#ffd166', glowColor: 'rgba(255, 209, 102, 0.3)' },
	'hip-hop': { label: 'Hip-Hop', color: '#efb95b', glowColor: 'rgba(239, 185, 91, 0.32)' },
	'r-b-and-soul': { label: 'R&B and Soul', color: '#c77dff', glowColor: 'rgba(199, 125, 255, 0.34)' },
	jazz: { label: 'Jazz', color: '#06d6a0', glowColor: 'rgba(6, 214, 160, 0.3)' },
	classical: { label: 'Classical', color: '#e9c46a', glowColor: 'rgba(233, 196, 106, 0.32)' },
	'folk-and-country': { label: 'Folk and Country', color: '#90be6d', glowColor: 'rgba(144, 190, 109, 0.3)' },
	latin: { label: 'Latin', color: '#f72585', glowColor: 'rgba(247, 37, 133, 0.34)' },
	'reggae-and-caribbean': { label: 'Reggae and Caribbean', color: '#43aa8b', glowColor: 'rgba(67, 170, 139, 0.3)' },
	blues: { label: 'Blues', color: '#4cc9f0', glowColor: 'rgba(76, 201, 240, 0.3)' },
	'ambient-and-experimental': {
		label: 'Ambient and Experimental',
		color: '#a8dadc',
		glowColor: 'rgba(168, 218, 220, 0.28)'
	},
	'soundtrack-and-screen': {
		label: 'Soundtrack and Screen',
		color: '#f4a261',
		glowColor: 'rgba(244, 162, 97, 0.3)'
	},
	world: { label: 'World', color: '#e76f51', glowColor: 'rgba(231, 111, 81, 0.3)' }
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
	// Phase 1: listening-driven topology
	orbitRadius: number;
	cohortId: string | null;
	// Phase 3: evolution data
	evolutionHistory: { periodStart: string; listenCount: number }[];
}

export interface GalaxyEdge {
	sourceId: number;
	targetId: number;
	type: 'parent-child' | 'sibling' | 'co-listening';
	weight: number;
}

export interface CoListeningEdge {
	genreA: number;
	genreB: number;
	coListenCount: number;
	jaccard: number;
}

export interface GalaxyCohort {
	id: string;
	label: string;
	icon: string;
	genreIds: number[];
	listenCount: number;
}

export interface GalaxyConfig {
	viewMode: 'map' | 'constellations' | 'mood' | 'heat' | 'paths';
	listeningDriven: boolean;
	showCohorts: boolean;
	showEvolution: boolean;
	showCoListening: boolean;
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
export type GalaxyViewMode = 'map' | 'constellations' | 'mood' | 'heat' | 'paths';

export const GALAXY_ROOT_RING_RADIUS = 380;
export const GALAXY_DEFAULT_SCALE = 0.58;

export function familyKeyFromSlug(slug: string): RootFamilyKey {
	if (slug in ROOT_FAMILY_COLORS) {
		return slug as RootFamilyKey;
	}
	return 'world';
}
