/**
 * Synthetic Energy × Danceability scatter for the SonicField preview.
 * Deterministic — seeded mulberry32 so reloads render the same shapes.
 */

import type { SonicView, SonicTrack } from '$lib/api/client';

export type SonicProfile = 'club' | 'eclectic' | 'chill' | 'aggressive';

interface Cluster {
	muE: number; // mean energy
	muD: number; // mean danceability
	muBpm: number;
	sE: number;
	sD: number;
	sBpm: number;
	count: number;
}

const PROFILES: Record<SonicProfile, Cluster[]> = {
	club: [
		// Concentrated upper-right: high energy, high danceability, club tempos.
		{ muE: 0.78, muD: 0.78, muBpm: 124, sE: 0.08, sD: 0.06, sBpm: 5, count: 220 },
		{ muE: 0.85, muD: 0.7, muBpm: 132, sE: 0.07, sD: 0.08, sBpm: 6, count: 120 },
		{ muE: 0.72, muD: 0.82, muBpm: 122, sE: 0.06, sD: 0.05, sBpm: 4, count: 90 },
	],
	eclectic: [
		{ muE: 0.32, muD: 0.42, muBpm: 88, sE: 0.1, sD: 0.12, sBpm: 8, count: 80 },
		{ muE: 0.55, muD: 0.62, muBpm: 100, sE: 0.1, sD: 0.1, sBpm: 7, count: 110 },
		{ muE: 0.74, muD: 0.78, muBpm: 122, sE: 0.08, sD: 0.07, sBpm: 6, count: 130 },
		{ muE: 0.86, muD: 0.55, muBpm: 142, sE: 0.06, sD: 0.12, sBpm: 8, count: 90 },
		{ muE: 0.62, muD: 0.28, muBpm: 168, sE: 0.12, sD: 0.1, sBpm: 10, count: 60 },
	],
	chill: [
		// Lower-left: low energy, mid-low danceability, downtempo.
		{ muE: 0.28, muD: 0.5, muBpm: 80, sE: 0.1, sD: 0.12, sBpm: 8, count: 180 },
		{ muE: 0.42, muD: 0.6, muBpm: 96, sE: 0.1, sD: 0.1, sBpm: 7, count: 140 },
		{ muE: 0.38, muD: 0.32, muBpm: 72, sE: 0.12, sD: 0.14, sBpm: 6, count: 80 },
	],
	aggressive: [
		// Bottom-right: high energy, low danceability, fast/intense.
		{ muE: 0.86, muD: 0.32, muBpm: 158, sE: 0.07, sD: 0.12, sBpm: 10, count: 180 },
		{ muE: 0.92, muD: 0.42, muBpm: 174, sE: 0.05, sD: 0.1, sBpm: 8, count: 130 },
		{ muE: 0.78, muD: 0.22, muBpm: 188, sE: 0.08, sD: 0.1, sBpm: 6, count: 70 },
	],
};

function mulberry32(seed: number): () => number {
	let a = seed;
	return () => {
		a |= 0;
		a = (a + 0x6d2b79f5) | 0;
		let t = a;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

function gaussian(rand: () => number, mu: number, sigma: number): number {
	const u = 1 - rand();
	const v = rand();
	const z = Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * v);
	return mu + z * sigma;
}

function clamp(x: number, lo: number, hi: number): number {
	return Math.max(lo, Math.min(hi, x));
}

const ARTISTS = [
	'Vesper Drive', 'Sable Echoes', 'Cold Geography', 'Private Wave', 'Night Terrace',
	'Inner Static', 'Hollow Signal', 'Distant Rooms', 'Static Bloom', 'After the Cut',
	'Quiet Forge', 'Pale Mirror', 'Silver Field', 'Long Wire', 'Arc Light',
];
const TITLE_WORDS_A = ['Pale', 'Silent', 'Hollow', 'Bright', 'Soft', 'Late', 'Endless', 'Distant', 'Quiet', 'Frayed'];
const TITLE_WORDS_B = ['Mirror', 'Signal', 'Bloom', 'Wave', 'Wire', 'Field', 'Room', 'Echo', 'Drift', 'Engine'];

export function generateDemoSonicField(profile: SonicProfile = 'club', seed = 42): SonicView {
	const rand = mulberry32(seed);
	const tracks: SonicTrack[] = [];
	let id = 1;
	for (const cluster of PROFILES[profile]) {
		for (let i = 0; i < cluster.count; i++) {
			const e = clamp(gaussian(rand, cluster.muE, cluster.sE), 0.02, 0.98);
			const d = clamp(gaussian(rand, cluster.muD, cluster.sD), 0.02, 0.98);
			const bpm = clamp(gaussian(rand, cluster.muBpm, cluster.sBpm), 60, 199);
			// Listens follow a heavy-tailed distribution — most tracks have 1–3 plays,
			// occasional outliers reach 30+.
			const u = rand();
			const listens = u < 0.7 ? 1 + Math.floor(rand() * 4) : 5 + Math.floor(rand() * rand() * 60);
			const titleA = TITLE_WORDS_A[Math.floor(rand() * TITLE_WORDS_A.length)];
			const titleB = TITLE_WORDS_B[Math.floor(rand() * TITLE_WORDS_B.length)];
			tracks.push({
				track_id: id++,
				title: `${titleA} ${titleB}`,
				artist_name: ARTISTS[Math.floor(rand() * ARTISTS.length)],
				album: null,
				artwork_path: null,
				file_path: null,
				e,
				d,
				bpm,
				listens,
			});
		}
	}
	return {
		tracks,
		total: tracks.length,
		coverage: { analyzed: tracks.length, total_listened: Math.round(tracks.length * 1.15) },
	};
}
