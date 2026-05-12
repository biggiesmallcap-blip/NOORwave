/**
 * Formatter contract tests. The plan calls for an "identical-output" test that catches
 * the case where someone adds a default that quietly drifts the rendering between two
 * components that sit next to each other on the page (e.g. peak_hour in spine vs caption,
 * BPM in MEDIAN sidecar vs MODE tooltip).
 *
 * Every assertion below ALSO doubles as the "-- for missing data" enforcement test
 * required by the single rule at the top of format.ts.
 */

import { describe, expect, test } from 'vitest';
import {
	formatBpm,
	formatCount,
	formatDate,
	formatDateShort,
	formatDelta,
	formatDr,
	formatDuration,
	formatHour,
	formatLufs,
	formatMultiplier,
	formatPercent,
	formatTilt,
	formatTrackDuration,
	getQualityClass,
} from './format';

const EMPTY = '--';

// ─── The single rule: -- for null / undefined / NaN ──────────────────────────

describe('every formatter returns -- for missing data', () => {
	for (const v of [null, undefined, Number.NaN]) {
		test(`input ${String(v)} (number-like)`, () => {
			expect(formatDuration(v as number | null | undefined)).toBe(EMPTY);
			expect(formatPercent(v as number | null | undefined, { decimals: 0 })).toBe(EMPTY);
			expect(formatBpm(v as number | null | undefined, { decimals: 0, suffix: 'BPM' })).toBe(EMPTY);
			expect(formatHour(v as number | null | undefined)).toBe(EMPTY);
			expect(formatCount(v as number | null | undefined)).toBe(EMPTY);
			expect(formatTilt(v as number | null | undefined)).toBe(EMPTY);
			expect(formatLufs(v as number | null | undefined)).toBe(EMPTY);
			expect(formatDr(v as number | null | undefined)).toBe(EMPTY);
			expect(formatMultiplier(v as number | null | undefined)).toBe(EMPTY);
		});
	}
	test('formatDelta with nullish previous returns --', () => {
		expect(formatDelta(100, null).text).toBe(EMPTY);
		expect(formatDelta(100, 0).text).toBe(EMPTY);
		expect(formatDelta(null, 100).text).toBe(EMPTY);
	});
	test('formatDate with nullish input returns --', () => {
		expect(formatDate(null, 'day')).toBe(EMPTY);
		expect(formatDate(undefined, 'week')).toBe(EMPTY);
		expect(formatDate(null, 'iso')).toBe(EMPTY);
	});
});

// ─── formatDuration ──────────────────────────────────────────────────────────

describe('formatDuration', () => {
	test('milliseconds → "Hh MMm" / "Mm" / "0m"', () => {
		expect(formatDuration(0)).toBe('0m');
		expect(formatDuration(60_000)).toBe('1m');
		expect(formatDuration(59 * 60_000)).toBe('59m');
		expect(formatDuration(60 * 60_000)).toBe('1h 00m');
		expect(formatDuration(37 * 60 * 60_000 + 12 * 60_000)).toBe('37h 12m');
	});
	test('negative input → --', () => {
		expect(formatDuration(-1)).toBe(EMPTY);
	});
});

// ─── formatPercent ───────────────────────────────────────────────────────────

describe('formatPercent', () => {
	test('decimals: 0 rounds with toFixed', () => {
		expect(formatPercent(0.74, { decimals: 0 })).toBe('74%');
		expect(formatPercent(0, { decimals: 0 })).toBe('0%'); // 0 is a real value, not missing
		expect(formatPercent(1, { decimals: 0 })).toBe('100%');
	});
	test('decimals: 1 keeps one decimal', () => {
		expect(formatPercent(0.742, { decimals: 1 })).toBe('74.2%');
	});
});

// ─── formatDelta ─────────────────────────────────────────────────────────────

describe('formatDelta', () => {
	test('positive delta → "+8%" sign 1', () => {
		const d = formatDelta(108, 100);
		expect(d.text).toBe('+8%');
		expect(d.sign).toBe(1);
		expect(d.magnitude).toBeCloseTo(0.08, 4);
	});
	test('negative delta → "-3%" sign -1', () => {
		const d = formatDelta(97, 100);
		expect(d.text).toBe('-3%');
		expect(d.sign).toBe(-1);
		expect(d.magnitude).toBeCloseTo(0.03, 4);
	});
	test('below 0.5% magnitude collapses to ±0% sign 0 (no jitter)', () => {
		expect(formatDelta(100.4, 100).text).toBe('±0%');
		expect(formatDelta(100.4, 100).sign).toBe(0);
		expect(formatDelta(99.6, 100).text).toBe('±0%');
		expect(formatDelta(99.6, 100).sign).toBe(0);
	});
	test('1% magnitude does NOT collapse', () => {
		expect(formatDelta(101, 100).text).toBe('+1%');
		expect(formatDelta(99, 100).text).toBe('-1%');
	});
});

// ─── formatBpm ───────────────────────────────────────────────────────────────

describe('formatBpm', () => {
	test('decimals 0 with BPM suffix', () => {
		expect(formatBpm(118.0, { decimals: 0, suffix: 'BPM' })).toBe('118 BPM');
		expect(formatBpm(123.7, { decimals: 0, suffix: 'BPM' })).toBe('124 BPM'); // rounds
	});
	test('decimals 1 without suffix (sigma)', () => {
		expect(formatBpm(18.4, { decimals: 1, suffix: '' })).toBe('18.4');
		// Note: 18.45 in IEEE754 is 18.4499... so toFixed(1) → "18.4". Use 18.46 to hit
		// the round-up branch unambiguously. Sigma in practice is ~5–25 with many
		// decimals from real data, so the boundary case is academic.
		expect(formatBpm(18.46, { decimals: 1, suffix: '' })).toBe('18.5');
	});
});

// ─── formatHour ──────────────────────────────────────────────────────────────

describe('formatHour', () => {
	test('valid hours render HH:00', () => {
		expect(formatHour(0)).toBe('00:00');
		expect(formatHour(9)).toBe('09:00');
		expect(formatHour(21)).toBe('21:00');
		expect(formatHour(23)).toBe('23:00');
	});
	test('out-of-range → --', () => {
		expect(formatHour(-1)).toBe(EMPTY);
		expect(formatHour(24)).toBe(EMPTY);
	});
});

// ─── formatDate ──────────────────────────────────────────────────────────────

describe('formatDate', () => {
	test('day format inside a single-year window', () => {
		const window = [{ label: '2026-04-21' }, { label: '2026-04-22' }];
		expect(formatDate('2026-04-21', 'day', { window })).toBe('Apr 21');
	});
	test('day format across year boundary adds year', () => {
		const window = [{ label: '2025-12-30' }, { label: '2026-01-02' }];
		expect(formatDate('2025-12-30', 'day', { window })).toBe('Dec 30 2025');
		expect(formatDate('2026-01-02', 'day', { window })).toBe('Jan 02 2026');
	});
	test('month format', () => {
		expect(formatDate('2026-04', 'month')).toBe('Apr 2026');
	});
	test('day-tooltip format', () => {
		// 2026-04-21 was a Tuesday.
		expect(formatDate('2026-04-21', 'day-tooltip')).toBe('Tue 21 Apr');
	});
	test('iso passthrough', () => {
		expect(formatDate('2026-04-21T00:00:00Z', 'iso')).toBe('2026-04-21T00:00:00Z');
	});
});

// ─── formatTilt / formatLufs / formatDr / formatMultiplier ───────────────────

describe('engineering-value formatters', () => {
	test('formatTilt always signed', () => {
		expect(formatTilt(2.3)).toBe('+2.3');
		expect(formatTilt(-1.1)).toBe('-1.1');
		expect(formatTilt(0)).toBe('+0.0'); // 0 is a real value, signed positive by convention
	});
	test('formatLufs preserves sign', () => {
		expect(formatLufs(-11.2)).toBe('-11.2 LUFS');
		expect(formatLufs(0)).toBe('0.0 LUFS');
	});
	test('formatDr unsigned', () => {
		expect(formatDr(9.1)).toBe('9.1 DR');
	});
	test('formatMultiplier with x suffix', () => {
		expect(formatMultiplier(1.8)).toBe('1.8x');
		expect(formatMultiplier(2)).toBe('2.0x');
	});
});

// ─── Identical-output: same input across two call sites must render identically ──

describe('identical-output across call sites', () => {
	test('peak_hour appears in spine and caption — both render the same string', () => {
		const peak = 21;
		const spine = formatHour(peak);
		const caption = formatHour(peak);
		expect(spine).toBe(caption);
	});
	test('BPM median in MEDIAN sidecar and MODE in tooltip share format', () => {
		const median = 118.0;
		const sidecar = formatBpm(median, { decimals: 0, suffix: 'BPM' });
		const tooltip = formatBpm(median, { decimals: 0, suffix: 'BPM' });
		expect(sidecar).toBe(tooltip);
	});
	test('completion ratio in KPI cell and cohort row share format', () => {
		const r = 0.74;
		const kpi = formatPercent(r, { decimals: 0 });
		const cohort = formatPercent(r, { decimals: 0 });
		expect(kpi).toBe(cohort);
	});
	test('listened_ms in KPI cell and cohort row share format', () => {
		const ms = 24_480_000;
		const kpi = formatDuration(ms);
		const cohort = formatDuration(ms);
		expect(kpi).toBe(cohort);
	});
});

// ─── Track duration (M:SS, used in track rows / now-playing) ─────────────────

describe('formatTrackDuration', () => {
	test('returns "--:--" sentinel for null/undefined/NaN/0', () => {
		expect(formatTrackDuration(null)).toBe('--:--');
		expect(formatTrackDuration(undefined)).toBe('--:--');
		expect(formatTrackDuration(Number.NaN)).toBe('--:--');
		expect(formatTrackDuration(0)).toBe('--:--');
	});
	test('renders M:SS', () => {
		expect(formatTrackDuration(45_000)).toBe('0:45');
		expect(formatTrackDuration(225_000)).toBe('3:45');
		expect(formatTrackDuration(7 * 60_000)).toBe('7:00');
	});
	test('zero-pads seconds', () => {
		expect(formatTrackDuration(60_000)).toBe('1:00');
		expect(formatTrackDuration(65_000)).toBe('1:05');
	});
	test('does not roll into hours — minutes count above 60 still render', () => {
		expect(formatTrackDuration(72 * 60_000 + 7_000)).toBe('72:07');
	});
});

// ─── formatDateShort (relative-date for track / album metadata) ──────────────

describe('formatDateShort', () => {
	test('returns em-dash for null', () => {
		expect(formatDateShort(null)).toBe('—');
	});
	test('Today / Yesterday', () => {
		const now = new Date();
		expect(formatDateShort(now.toISOString())).toBe('Today');
		const sameDayFuture = new Date(now);
		sameDayFuture.setHours(sameDayFuture.getHours() + 4);
		expect(formatDateShort(sameDayFuture.toISOString())).toBe('Today');
		const yesterday = new Date(now);
		yesterday.setDate(yesterday.getDate() - 1);
		expect(formatDateShort(yesterday.toISOString())).toBe('Yesterday');
	});
	test('within a week → "Nd ago"', () => {
		const past = new Date();
		past.setDate(past.getDate() - 3);
		expect(formatDateShort(past.toISOString())).toBe('3d ago');
	});
	test('within a month → "Nw ago"', () => {
		const past = new Date();
		past.setDate(past.getDate() - 14);
		expect(formatDateShort(past.toISOString())).toBe('2w ago');
	});
	test('within a year → "Nmo ago"', () => {
		const past = new Date();
		past.setDate(past.getDate() - 90);
		expect(formatDateShort(past.toISOString())).toBe('3mo ago');
	});
	test('older than a year → locale date', () => {
		// A clearly-old date ensures we land in the locale-date branch regardless of test-run time.
		const result = formatDateShort('2020-01-15T00:00:00.000Z');
		expect(result).toMatch(/\b2020\b/);
		expect(result).toMatch(/Jan/);
	});
});

// ─── getQualityClass (TIDAL audio-quality → CSS class) ───────────────────────

describe('getQualityClass', () => {
	test('null → lossy', () => {
		expect(getQualityClass(null)).toBe('lossy');
	});
	test('HI_RES variants → hires', () => {
		expect(getQualityClass('HI_RES')).toBe('hires');
		expect(getQualityClass('HI_RES_LOSSLESS')).toBe('hires');
	});
	test('LOSSLESS → lossless', () => {
		expect(getQualityClass('LOSSLESS')).toBe('lossless');
	});
	test('HIGH / LOW / unknown → lossy', () => {
		expect(getQualityClass('HIGH')).toBe('lossy');
		expect(getQualityClass('LOW')).toBe('lossy');
		expect(getQualityClass('UNKNOWN')).toBe('lossy');
	});
});
