/**
 * Shared formatters for the analytics page (and other surfaces that want consistent output).
 *
 * THE SINGLE RULE
 *   Every formatter returns "--" for null, undefined, or NaN inputs. No formatter ever
 *   returns "0%", "0", "0h 0m", "0 BPM", or any other zero-valued string for missing data —
 *   those strings are reserved for actual zeros. This single rule prevents the
 *   "is this user inactive or is the page broken?" class of bug.
 *
 * Contract: frontend/src/lib/utils/format.test.ts
 */

const EMPTY = '--';

function isMissing(v: number | null | undefined): v is null | undefined {
	return v === null || v === undefined || (typeof v === 'number' && Number.isNaN(v));
}

function isMissingStr(v: string | null | undefined): v is null | undefined {
	return v === null || v === undefined;
}

// ─── Duration ────────────────────────────────────────────────────────────────

/** Duration in milliseconds → "37h 12m" or "12m" or "0m". null/NaN → "--". */
export function formatDuration(ms: number | null | undefined): string {
	if (isMissing(ms)) return EMPTY;
	if (ms < 0) return EMPTY;
	const minutes = Math.floor(ms / 60000);
	if (minutes < 60) return `${minutes}m`;
	const hours = Math.floor(minutes / 60);
	const remMin = minutes % 60;
	return `${hours}h ${String(remMin).padStart(2, '0')}m`;
}

/**
 * Track duration in milliseconds → "M:SS" (e.g. "3:45", "12:07").
 * null/0/NaN → "--:--" (clock-shaped placeholder for track-row visual continuity;
 * this differs deliberately from the analytics-family `--` empty sentinel).
 */
export function formatTrackDuration(ms: number | null | undefined): string {
	if (!ms || isMissing(ms)) return '--:--';
	const totalSeconds = Math.floor(ms / 1000);
	const minutes = Math.floor(totalSeconds / 60);
	const seconds = totalSeconds % 60;
	return `${minutes}:${seconds.toString().padStart(2, '0')}`;
}

export function formatTotalDuration(ms: number | null | undefined): string {
	if (isMissing(ms) || ms < 0) return EMPTY;
	const minutes = Math.round(ms / 60000);
	if (minutes < 60) return `${minutes} min`;
	const hours = Math.floor(minutes / 60);
	const remaining = minutes % 60;
	return remaining ? `${hours} hr ${remaining} min` : `${hours} hr`;
}

/**
 * Relative-date formatter for track / album "added" timestamps.
 *
 *   formatDateShort('2026-05-08T...') → "Today"
 *   formatDateShort('2026-05-05T...') → "3d ago"
 *   formatDateShort('2025-12-01T...') → "Dec 1, 2025"
 *   formatDateShort(null)             → "—"
 *
 * Uses an em-dash for the empty sentinel because it appears beside formatted
 * dates in track-row metadata, not in analytics tiles.
 */
export function formatDateShort(iso: string | null): string {
	if (!iso) return '—';
	const normalized = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/.test(iso)
		? `${iso.replace(' ', 'T')}Z`
		: iso;
	const d = new Date(normalized);
	const now = new Date();
	const diffMs = now.getTime() - d.getTime();
	const diffDays = Math.floor(diffMs / 86400000);

	if (diffDays <= 0) return 'Today';
	if (diffDays === 1) return 'Yesterday';
	if (diffDays < 7) return `${diffDays}d ago`;
	if (diffDays < 30) return `${Math.floor(diffDays / 7)}w ago`;
	if (diffDays < 365) return `${Math.floor(diffDays / 30)}mo ago`;
	return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' });
}

/** TIDAL audio-quality string → CSS class name for `.quality-badge.<class>`. */
export function getQualityClass(quality: string | null): 'hires' | 'lossless' | 'lossy' {
	if (!quality) return 'lossy';
	if (quality.includes('HI_RES')) return 'hires';
	if (quality === 'LOSSLESS') return 'lossless';
	return 'lossy';
}

// ─── Percent / delta ─────────────────────────────────────────────────────────

/** Ratio 0..1 → "74%". null → "--". `decimals` is required at every call site (no default). */
export function formatPercent(
	ratio: number | null | undefined,
	opts: { decimals: 0 | 1 },
): string {
	if (isMissing(ratio)) return EMPTY;
	const pct = ratio * 100;
	return `${pct.toFixed(opts.decimals)}%`;
}

/**
 * Compare two numeric snapshots — returns text + sign + raw magnitude.
 *
 *   formatDelta(108, 100) → { text: "+8%", sign: 1, magnitude: 0.08 }
 *   formatDelta(96,  100) → { text: "-4%", sign: -1, magnitude: 0.04 }
 *
 * Below 0.005 magnitude (0.5%) the rendering collapses to "±0%" with sign 0 to avoid
 * "+0%"/"-0%" jitter. Returns "--" with sign 0 when previous is zero or nullish.
 */
export function formatDelta(
	current: number | null | undefined,
	previous: number | null | undefined,
): { text: string; sign: -1 | 0 | 1; magnitude: number } {
	if (isMissing(current) || isMissing(previous) || previous === 0) {
		return { text: EMPTY, sign: 0, magnitude: 0 };
	}
	const delta = (current - previous) / previous;
	const magnitude = Math.abs(delta);
	if (magnitude < 0.005) {
		return { text: '±0%', sign: 0, magnitude };
	}
	const sign = delta > 0 ? 1 : -1;
	const pct = Math.round(magnitude * 100);
	return {
		text: `${sign > 0 ? '+' : '-'}${pct}%`,
		sign: sign as -1 | 1,
		magnitude,
	};
}

// ─── BPM ─────────────────────────────────────────────────────────────────────

/**
 * BPM display. `decimals` is required at every call site (no default — anyone who
 * forgets would silently get integer rendering, which is wrong for sigma).
 *
 *   formatBpm(118.0, { decimals: 0, suffix: "BPM" }) → "118 BPM"
 *   formatBpm(18.4,  { decimals: 1, suffix: "" })    → "18.4"
 */
export function formatBpm(
	bpm: number | null | undefined,
	opts: { decimals: 0 | 1; suffix: 'BPM' | '' },
): string {
	if (isMissing(bpm)) return EMPTY;
	const value = bpm.toFixed(opts.decimals);
	return opts.suffix ? `${value} ${opts.suffix}` : value;
}

// ─── Hour-of-day ─────────────────────────────────────────────────────────────

/** Hour-of-day 0..23 → "21:00". 0 is a valid input (returns "00:00"). null → "--". */
export function formatHour(hour: number | null | undefined): string {
	if (isMissing(hour)) return EMPTY;
	if (hour < 0 || hour > 23) return EMPTY;
	return `${String(Math.floor(hour)).padStart(2, '0')}:00`;
}

// ─── Date ────────────────────────────────────────────────────────────────────

/**
 * Date formatting per granularity used by the analytics page.
 *
 *   "day"          → "Apr 21" (default) or "Apr 21 2026" if the row-set spans a year boundary.
 *                    Pass the full row-set via `opts.window` so the year-boundary decision
 *                    happens once at the row-set level, uniform across all rows.
 *   "week"         → "May 19"   input "YYYY-UU" (SQLite %U: Sunday-start). Resolves to
 *                                the Sunday-start date.
 *   "month"        → "Apr 2026" input "YYYY-MM"
 *   "day-tooltip"  → "Tue 21 Apr"   input ISO date — used inside the ridgeline tooltip
 *   "iso"          → returns input untouched
 */
export function formatDate(
	value: string | null | undefined,
	granularity: 'day' | 'week' | 'month' | 'day-tooltip' | 'iso',
	opts?: { window?: { label: string }[] },
): string {
	if (isMissingStr(value)) return EMPTY;

	if (granularity === 'iso') return value;

	if (granularity === 'month') {
		// "YYYY-MM"
		const m = /^(\d{4})-(\d{2})$/.exec(value);
		if (!m) return value;
		const year = Number(m[1]);
		const monthIdx = Number(m[2]) - 1;
		return `${MONTH_SHORT[monthIdx] ?? '???'} ${year}`;
	}

	if (granularity === 'week') {
		// "YYYY-UU" → resolve to Sunday-start date.
		const m = /^(\d{4})-(\d{2})$/.exec(value);
		if (!m) return value;
		const year = Number(m[1]);
		const weekU = Number(m[2]);
		const date = sundayStartFromYearWeekU(year, weekU);
		return formatDayLike(date);
	}

	// "day" or "day-tooltip" — input is an ISO date "YYYY-MM-DD".
	const date = new Date(`${value}T00:00:00`);
	if (Number.isNaN(date.getTime())) return value;

	if (granularity === 'day-tooltip') {
		const wd = WEEKDAY_SHORT[date.getDay()];
		const dd = date.getDate();
		const mon = MONTH_SHORT[date.getMonth()];
		return `${wd} ${dd} ${mon}`;
	}

	// "day": include year iff the window straddles a year boundary.
	const includeYear = opts?.window ? windowStraddlesYear(opts.window) : false;
	return formatDayLike(date, includeYear);
}

function formatDayLike(date: Date, includeYear = false): string {
	const dd = String(date.getDate()).padStart(2, '0');
	const mon = MONTH_SHORT[date.getMonth()];
	if (includeYear) return `${mon} ${dd} ${date.getFullYear()}`;
	return `${mon} ${dd}`;
}

function windowStraddlesYear(window: { label: string }[]): boolean {
	if (window.length < 2) return false;
	const years = new Set<string>();
	for (const r of window) {
		const m = /^(\d{4})/.exec(r.label);
		if (m) years.add(m[1]);
	}
	return years.size > 1;
}

/**
 * SQLite %U weeks start Sunday and number 00–53. Week 00 contains days from
 * Jan 1 up to (but not including) the first Sunday of the year. This resolver
 * returns the Sunday that opens the requested week.
 */
function sundayStartFromYearWeekU(year: number, weekU: number): Date {
	const jan1 = new Date(year, 0, 1);
	// Day-of-week 0 (Sun) ... 6 (Sat).
	const jan1Day = jan1.getDay();
	// First Sunday of the year (start of week 01). For week 00 we use Jan 1 itself.
	const firstSunday = new Date(year, 0, 1 + ((7 - jan1Day) % 7));
	if (weekU === 0) return jan1;
	return new Date(year, 0, firstSunday.getDate() + (weekU - 1) * 7);
}

const MONTH_SHORT = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const WEEKDAY_SHORT = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

// ─── Counts and engineering values ───────────────────────────────────────────

/** Integer count with locale separators → "2,413" / "847". null → "--". */
export function formatCount(value: number | null | undefined): string {
	if (isMissing(value)) return EMPTY;
	return Math.round(value).toLocaleString();
}

export function formatCompactCount(value: number | null | undefined): string {
	if (isMissing(value)) return EMPTY;
	const rounded = Math.round(value);
	if (rounded >= 1_000_000_000) return `${(rounded / 1_000_000_000).toFixed(1)}B`;
	if (rounded >= 1_000_000) return `${(rounded / 1_000_000).toFixed(1)}M`;
	if (rounded >= 1_000) return `${(rounded / 1_000).toFixed(1)}K`;
	return rounded.toLocaleString();
}

/** Signed tilt → "+2.3" / "-1.1" (one decimal, always signed). Used for BASS/TREBLE TILT. */
export function formatTilt(value: number | null | undefined): string {
	if (isMissing(value)) return EMPTY;
	const rounded = value.toFixed(1);
	return value >= 0 ? `+${rounded}` : rounded;
}

/** Loudness LUFS → "-11.2 LUFS" (one decimal, sign preserved). */
export function formatLufs(value: number | null | undefined): string {
	if (isMissing(value)) return EMPTY;
	return `${value.toFixed(1)} LUFS`;
}

/** Dynamic range → "9.1 DR" (one decimal, unsigned). */
export function formatDr(value: number | null | undefined): string {
	if (isMissing(value)) return EMPTY;
	return `${value.toFixed(1)} DR`;
}

/** Multiplier → "1.8x" (one decimal, x suffix). Used for cohort REPEAT RATE. */
export function formatMultiplier(value: number | null | undefined): string {
	if (isMissing(value)) return EMPTY;
	return `${value.toFixed(1)}x`;
}
