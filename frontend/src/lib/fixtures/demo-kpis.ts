/**
 * Synthetic SignalsKpis for the strip preview. Produces 2N days of plausible
 * daily activity (current window + previous window for delta computation).
 *
 * Deterministic — seeded mulberry32 so reloads render the same numbers.
 */

import type { SignalsKpis, DailyKpi, HeroStats, SessionsCoverage } from '$lib/api/client';

export type KpiProfile = 'casual' | 'heavy' | 'sporadic' | 'recovering';

interface ProfileSpec {
	dailyMean: number; // typical daily listens
	dailySd: number;
	completionMean: number; // 0..1
	completionSd: number;
	avgTrackMs: number; // typical played-ms per listen
	driftPct: number; // how much current window trends vs previous (-0.4 .. +0.4)
	offDayProb: number;
}

const PROFILES: Record<KpiProfile, ProfileSpec> = {
	casual: { dailyMean: 32, dailySd: 22, completionMean: 0.74, completionSd: 0.10, avgTrackMs: 150_000, driftPct: 0.08, offDayProb: 0.12 },
	heavy: { dailyMean: 120, dailySd: 55, completionMean: 0.82, completionSd: 0.07, avgTrackMs: 180_000, driftPct: 0.05, offDayProb: 0.06 },
	sporadic: { dailyMean: 18, dailySd: 28, completionMean: 0.52, completionSd: 0.14, avgTrackMs: 110_000, driftPct: -0.12, offDayProb: 0.30 },
	recovering: { dailyMean: 60, dailySd: 32, completionMean: 0.78, completionSd: 0.10, avgTrackMs: 165_000, driftPct: 0.32, offDayProb: 0.08 },
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

function dateNDaysAgo(n: number): string {
	const d = new Date();
	d.setHours(0, 0, 0, 0);
	d.setDate(d.getDate() - n);
	return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

export function generateDemoKpis(
	profile: KpiProfile = 'casual',
	days = 30,
	seed = 42,
): SignalsKpis {
	const rand = mulberry32(seed);
	const spec = PROFILES[profile];

	// Apply drift: current window's mean is higher (or lower) than previous.
	// drift of +0.08 means current is 8% above the profile mean, previous is 8% below.
	const curMean = spec.dailyMean * (1 + spec.driftPct);
	const prevMean = spec.dailyMean * (1 - spec.driftPct);

	function dayFor(rangeMean: number, dayOffset: number): DailyKpi {
		const date = dateNDaysAgo(dayOffset);
		const off = rand() < spec.offDayProb;
		if (off) return { day: date, listens: 0, listened_ms: 0, completed: 0 };
		const listens = Math.max(0, Math.round(gaussian(rand, rangeMean, spec.dailySd)));
		const completionToday = Math.max(0.1, Math.min(0.97, gaussian(rand, spec.completionMean, spec.completionSd)));
		const completed = Math.round(listens * completionToday);
		const listened_ms = Math.round(listens * spec.avgTrackMs * (0.5 + rand() * 1.1));
		return { day: date, listens, listened_ms, completed };
	}

	// Current window: days 0..days-1 (today first)
	const current: DailyKpi[] = [];
	for (let i = days - 1; i >= 0; i--) current.push(dayFor(curMean, i));
	// Previous window: days days..2*days-1
	const previous: DailyKpi[] = [];
	for (let i = 2 * days - 1; i >= days; i--) previous.push(dayFor(prevMean, i));

	const aggregate = (rows: DailyKpi[]) => {
		const listens = rows.reduce((s, r) => s + r.listens, 0);
		const listened_ms = rows.reduce((s, r) => s + r.listened_ms, 0);
		const completed = rows.reduce((s, r) => s + r.completed, 0);
		// Sessions ≈ active days × 1.4 (rough: listeners typically have ~1-2 sessions/day).
		const sessions = rows.filter((r) => r.listens > 0).length * 1 + Math.floor(rows.reduce((s, r) => s + r.listens, 0) / 24);
		const completionRatio = listens === 0 ? null : completed / listens;
		return { listens, listened_ms, completed, sessions, completionRatio };
	};

	const cur = aggregate(current);
	const prev = aggregate(previous);

	const heroStats: HeroStats = {
		peak_hour: 19 + Math.floor(rand() * 4) - 2, // somewhere 17..22
		rhythm: current.filter((r) => r.listens > 0).length < 5 ? null : Math.round(48 + rand() * 40),
		night_share: 0.18 + rand() * 0.22,
		morning_share: 0.08 + rand() * 0.18,
	};

	const sessions_coverage: SessionsCoverage = {
		tracked: cur.listens,
		untracked: 0,
	};

	return {
		listened_ms: { current: cur.listened_ms, previous: prev.listened_ms },
		sessions: { current: cur.sessions, previous: prev.sessions },
		completion: { current: cur.completionRatio, previous: prev.completionRatio },
		skip_rate: {
			current: cur.completionRatio === null ? null : 1 - cur.completionRatio,
			previous: prev.completionRatio === null ? null : 1 - prev.completionRatio,
		},
		daily: current,
		hero_stats: heroStats,
		sessions_coverage,
	};
}
