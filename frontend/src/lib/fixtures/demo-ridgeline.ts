/**
 * Synthetic ridgeline + hero-stats generators for the preview route.
 *
 * Real DBs need months of regular listening before the JD aesthetic emerges. These
 * generators produce plausible 30-day patterns so we can tune sigma / amp / opacity
 * against shapes we'd expect to see in a populated library.
 *
 * Deterministic — seeded mulberry32 so reloads render the same shapes.
 */

import type { HeroStats, RidgeRow } from '$lib/api/client';

export type DemoProfile = 'routine' | 'weekend-heavy' | 'late-night-owl' | 'casual';

export interface DemoSignals {
	ridgeline: RidgeRow[];
	heroStats: HeroStats;
	ridgeAmpMax: number;
}

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

function dateNDaysAgo(n: number): string {
	const d = new Date();
	d.setHours(0, 0, 0, 0);
	d.setDate(d.getDate() - n);
	const y = d.getFullYear();
	const m = String(d.getMonth() + 1).padStart(2, '0');
	const day = String(d.getDate()).padStart(2, '0');
	return `${y}-${m}-${day}`;
}

function gaussian(rand: () => number, mean: number, sd: number): number {
	// Box-Muller
	const u = 1 - rand();
	const v = rand();
	const z = Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * v);
	return mean + z * sd;
}

interface PeakSpec {
	hour: number;
	weight: number; // mean listens at peak
	width: number; // hour-spread
}

function dayShape(rand: () => number, peaks: PeakSpec[], offDayProb = 0.08): number[] {
	if (rand() < offDayProb) return new Array<number>(24).fill(0);
	const hourly = new Array<number>(24).fill(0);
	for (const p of peaks) {
		// Each peak contributes a Gaussian envelope of listen-counts.
		const hourCenter = p.hour + (rand() - 0.5) * 1.2; // small jitter
		const peakIntensity = p.weight * (0.6 + rand() * 0.8);
		for (let h = 0; h < 24; h++) {
			const dist = Math.min(Math.abs(h - hourCenter), 24 - Math.abs(h - hourCenter));
			const contribution =
				peakIntensity * Math.exp(-(dist * dist) / (2 * p.width * p.width));
			// Discrete listens — round, never negative.
			hourly[h] += Math.max(0, Math.round(contribution));
		}
	}
	// Sprinkle a couple of low-noise listens elsewhere.
	const noise = Math.floor(rand() * 4);
	for (let i = 0; i < noise; i++) {
		const h = Math.floor(rand() * 24);
		hourly[h] += 1;
	}
	return hourly;
}

const PROFILES: Record<DemoProfile, { weekday: PeakSpec[]; weekend: PeakSpec[]; offProb: number }> = {
	routine: {
		// Morning commute, lunch, evening wind-down.
		weekday: [
			{ hour: 8, weight: 6, width: 1.3 },
			{ hour: 13, weight: 3, width: 1.0 },
			{ hour: 20, weight: 9, width: 2.0 },
		],
		weekend: [
			{ hour: 11, weight: 5, width: 2.5 },
			{ hour: 17, weight: 7, width: 2.5 },
			{ hour: 22, weight: 5, width: 1.8 },
		],
		offProb: 0.05,
	},
	'weekend-heavy': {
		weekday: [
			{ hour: 9, weight: 2, width: 1.5 },
			{ hour: 19, weight: 4, width: 1.8 },
		],
		weekend: [
			{ hour: 14, weight: 12, width: 3.0 },
			{ hour: 22, weight: 18, width: 2.5 },
			{ hour: 1, weight: 8, width: 1.5 },
		],
		offProb: 0.12,
	},
	'late-night-owl': {
		weekday: [
			{ hour: 22, weight: 8, width: 1.5 },
			{ hour: 1, weight: 10, width: 1.8 },
			{ hour: 3, weight: 5, width: 1.5 },
		],
		weekend: [
			{ hour: 0, weight: 14, width: 2.5 },
			{ hour: 3, weight: 9, width: 2.0 },
		],
		offProb: 0.10,
	},
	casual: {
		weekday: [
			{ hour: 18, weight: 4, width: 2.5 },
		],
		weekend: [
			{ hour: 15, weight: 5, width: 3.5 },
			{ hour: 21, weight: 3, width: 2.0 },
		],
		offProb: 0.25,
	},
};

export function generateDemoRidgeline(
	profile: DemoProfile = 'routine',
	days = 30,
	seed = 42,
): DemoSignals {
	const rand = mulberry32(seed);
	const spec = PROFILES[profile];
	const ridgeline: RidgeRow[] = [];

	for (let i = days - 1; i >= 0; i--) {
		const date = dateNDaysAgo(i);
		const dow = new Date(`${date}T00:00:00`).getDay(); // 0 = Sun ... 6 = Sat
		const isWeekend = dow === 0 || dow === 6;
		const peaks = isWeekend ? spec.weekend : spec.weekday;
		const hourly = dayShape(rand, peaks, spec.offProb);
		ridgeline.push({ date, hourly });
	}

	// Hero stats derived from the synthesised data.
	const totalsByHour = new Array<number>(24).fill(0);
	let totalListens = 0;
	for (const row of ridgeline) {
		for (let h = 0; h < 24; h++) {
			totalsByHour[h] += row.hourly[h];
			totalListens += row.hourly[h];
		}
	}
	const peakHour =
		totalListens === 0
			? null
			: totalsByHour.reduce((best, v, h) => (v > totalsByHour[best] ? h : best), 0);

	const NIGHT = [22, 23, 0, 1, 2, 3, 4];
	const MORNING = [5, 6, 7, 8, 9];
	const nightSum = NIGHT.reduce((s, h) => s + totalsByHour[h], 0);
	const morningSum = MORNING.reduce((s, h) => s + totalsByHour[h], 0);
	const nightShare = totalListens === 0 ? null : nightSum / totalListens;
	const morningShare = totalListens === 0 ? null : morningSum / totalListens;

	// Rhythm via the same CV form the backend uses.
	const activeDays = ridgeline.filter((r) => r.hourly.some((v) => v > 0));
	const rhythm =
		activeDays.length < 5
			? null
			: (() => {
					let sigmaSum = 0;
					let listensSum = 0;
					for (const r of activeDays) {
						const mean = r.hourly.reduce((a, b) => a + b, 0) / 24;
						const variance =
							r.hourly.reduce((a, h) => a + (h - mean) ** 2, 0) / 24;
						sigmaSum += Math.sqrt(variance);
						listensSum += mean * 24;
					}
					const meanSigma = sigmaSum / activeDays.length;
					const meanListens = listensSum / (activeDays.length * 24);
					if (meanListens === 0) return 0;
					const cv = meanSigma / meanListens;
					return Math.round(100 * Math.max(0, Math.min(1, 1 - cv)));
				})();

	// ridge_amp_max = P95 across all per-row per-hour values.
	const flat = ridgeline.flatMap((r) => r.hourly).sort((a, b) => a - b);
	const p95 = flat.length === 0 ? 0 : flat[Math.floor(0.95 * (flat.length - 1))];

	return {
		ridgeline,
		heroStats: {
			peak_hour: peakHour,
			rhythm,
			night_share: nightShare,
			morning_share: morningShare,
		},
		ridgeAmpMax: p95,
	};
}
