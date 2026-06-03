/**
 * Synthetic tempo data for the TempoRidges preview route. Produces realistic
 * per-row BPM distributions (mixture-of-gaussians around tempo cluster centres)
 * so we can tune the chart against dense data without needing a populated DB.
 *
 * Deterministic — seeded mulberry32 so reloads render the same shapes.
 */

import type { TempoView, TempoRow, BpmBucket } from '$lib/api/client';
import { dateNDaysAgo, gaussian, mulberry32 } from '$lib/fixtures/demo-random';

export type TempoProfile = 'house-techno' | 'downtempo' | 'eclectic' | 'pop-radio';

const BPM_MIN = 60;
const BPM_MAX = 200;
const BPM_STEP = 4;
const BPM_BUCKETS = (BPM_MAX - BPM_MIN) / BPM_STEP; // 35 buckets covering [60, 196]

interface ProfileSpec {
	clusters: { mu: number; sigma: number; weight: number }[];
	listensPerRow: { mean: number; sd: number };
	emptyRowProb: number;
}

const PROFILES: Record<TempoProfile, ProfileSpec> = {
	'house-techno': {
		// Heavy clustering at house tempos with secondary at deeper techno.
		clusters: [
			{ mu: 122, sigma: 5, weight: 0.5 },
			{ mu: 128, sigma: 4, weight: 0.3 },
			{ mu: 138, sigma: 6, weight: 0.2 },
		],
		listensPerRow: { mean: 28, sd: 12 },
		emptyRowProb: 0.06,
	},
	downtempo: {
		clusters: [
			{ mu: 84, sigma: 8, weight: 0.55 },
			{ mu: 100, sigma: 6, weight: 0.3 },
			{ mu: 124, sigma: 7, weight: 0.15 },
		],
		listensPerRow: { mean: 18, sd: 8 },
		emptyRowProb: 0.10,
	},
	eclectic: {
		// Wide spread with multiple modes — like a curious listener with broad taste.
		clusters: [
			{ mu: 78, sigma: 8, weight: 0.2 },
			{ mu: 100, sigma: 8, weight: 0.2 },
			{ mu: 122, sigma: 6, weight: 0.3 },
			{ mu: 140, sigma: 7, weight: 0.2 },
			{ mu: 168, sigma: 8, weight: 0.1 },
		],
		listensPerRow: { mean: 22, sd: 10 },
		emptyRowProb: 0.07,
	},
	'pop-radio': {
		clusters: [
			{ mu: 96, sigma: 7, weight: 0.3 },
			{ mu: 118, sigma: 5, weight: 0.5 },
			{ mu: 132, sigma: 6, weight: 0.2 },
		],
		listensPerRow: { mean: 32, sd: 10 },
		emptyRowProb: 0.04,
	},
};

function pickCluster(rand: () => number, clusters: ProfileSpec['clusters']): ProfileSpec['clusters'][number] {
	const r = rand();
	let cum = 0;
	for (const c of clusters) {
		cum += c.weight;
		if (r <= cum) return c;
	}
	return clusters[clusters.length - 1];
}

function denseBuckets(): BpmBucket[] {
	return Array.from({ length: BPM_BUCKETS }, (_, i) => ({
		bucket: BPM_MIN + i * BPM_STEP,
		listens: 0,
	}));
}

function weekStartISO(n: number): string {
	// Sunday-start year-week label "YYYY-UU" matching the backend's strftime('%Y-%U', ...).
	const d = new Date();
	d.setHours(0, 0, 0, 0);
	d.setDate(d.getDate() - n);
	const year = d.getFullYear();
	const start = new Date(year, 0, 1);
	const dayOfYear = Math.floor((d.getTime() - start.getTime()) / 86400000);
	const week = Math.floor((dayOfYear + start.getDay()) / 7);
	return `${year}-${String(week).padStart(2, '0')}`;
}

function monthLabel(n: number): string {
	const d = new Date();
	d.setHours(0, 0, 0, 0);
	d.setDate(d.getDate() - n);
	return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`;
}

/**
 * Mirrors the backend's `select_granularity` rule:
 *   1..=30  → Day
 *   31..=90 → Week
 *   _       → Month
 */
function tempoGranularity(days: number): 'day' | 'week' | 'month' {
	if (days <= 30) return 'day';
	if (days <= 90) return 'week';
	return 'month';
}

export function generateDemoTempo(
	profile: TempoProfile = 'house-techno',
	days = 30,
	seed = 42,
): TempoView {
	const rand = mulberry32(seed);
	const spec = PROFILES[profile];
	const granularity = tempoGranularity(days);

	// Number of rows + days-per-row depending on granularity.
	const { rowCount, daysPerRow } =
		granularity === 'day'
			? { rowCount: days, daysPerRow: 1 }
			: granularity === 'week'
				? { rowCount: Math.ceil(days / 7), daysPerRow: 7 }
				: { rowCount: Math.min(24, Math.ceil(days / 30)), daysPerRow: 30 };

	const rows: TempoRow[] = [];
	for (let i = 0; i < rowCount; i++) {
		// Most-recent row is at i = rowCount-1; we render top-to-bottom oldest-to-newest.
		const dayOffset = (rowCount - 1 - i) * daysPerRow;
		let label: string;
		if (granularity === 'day') label = dateNDaysAgo(dayOffset);
		else if (granularity === 'week') label = weekStartISO(dayOffset);
		else label = monthLabel(dayOffset);

		const buckets = denseBuckets();

		if (rand() >= spec.emptyRowProb) {
			// Sample N listens from the mixture of gaussians, scaled up for week/month rows.
			const baseN = Math.max(0, gaussian(rand, spec.listensPerRow.mean, spec.listensPerRow.sd));
			const n = Math.round(baseN * daysPerRow);
			for (let s = 0; s < n; s++) {
				const cluster = pickCluster(rand, spec.clusters);
				const bpm = gaussian(rand, cluster.mu, cluster.sigma);
				if (bpm < BPM_MIN || bpm >= BPM_MAX) continue;
				const idx = Math.min(BPM_BUCKETS - 1, Math.floor((bpm - BPM_MIN) / BPM_STEP));
				buckets[idx].listens += 1;
			}
		}

		rows.push({ label, granularity, buckets });
	}

	// Stats over the per-listen vector aggregated across rows.
	const weighted: { bpm: number; listens: number }[] = [];
	for (const row of rows) {
		for (const b of row.buckets) {
			if (b.listens > 0) {
				weighted.push({ bpm: b.bucket + BPM_STEP / 2, listens: b.listens });
			}
		}
	}
	const totalListens = weighted.reduce((s, w) => s + w.listens, 0);
	const median = weightedMedian(weighted);
	const mode = weighted.length === 0 ? null : weighted.reduce((m, w) => (w.listens > m.listens ? w : m)).bpm;
	const sigma = weightedStddev(weighted);

	// ridge_amp_max — P95 across per-row per-bucket values.
	const flat: number[] = [];
	for (const row of rows) for (const b of row.buckets) flat.push(b.listens);
	flat.sort((a, b) => a - b);
	const p95 = flat.length === 0 ? 0 : flat[Math.floor(0.95 * (flat.length - 1))];

	const analyzed = totalListens;
	const total_listened = Math.round(totalListens * 1.18); // pretend ~18% of listens lacked DSP
	return {
		bucket_axis: { min: BPM_MIN, max: BPM_MAX, step: BPM_STEP },
		rows,
		stats: { median, mode, sigma },
		coverage: { analyzed, total_listened },
		ridge_amp_max: p95,
	};
}

function weightedMedian(weighted: { bpm: number; listens: number }[]): number | null {
	if (weighted.length === 0) return null;
	const sorted = [...weighted].sort((a, b) => a.bpm - b.bpm);
	const total = sorted.reduce((s, w) => s + w.listens, 0);
	if (total === 0) return null;
	let cum = 0;
	for (const w of sorted) {
		cum += w.listens;
		if (cum >= total / 2) return w.bpm;
	}
	return sorted[sorted.length - 1].bpm;
}

function weightedStddev(weighted: { bpm: number; listens: number }[]): number | null {
	const total = weighted.reduce((s, w) => s + w.listens, 0);
	if (total < 2) return null;
	const mean = weighted.reduce((s, w) => s + w.bpm * w.listens, 0) / total;
	const variance =
		weighted.reduce((s, w) => s + (w.bpm - mean) ** 2 * w.listens, 0) / total;
	return Math.sqrt(variance);
}
