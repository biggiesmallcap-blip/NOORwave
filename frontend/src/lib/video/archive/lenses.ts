// Archive lenses: the filter state of /videos/archive and its two mirrors,
// the URL query string (so the Atlas can deep link `?scene=12` and Back
// restores the view) and the API query. Pure functions, no DOM.

import type {
	ArchiveDepth,
	ArchiveLength,
	ArchiveQuery,
	ArchiveSort,
	ArchiveSource,
	VideoCut,
} from '$lib/api/video_explore_types';

export interface ArchiveLenses {
	scene: number | null;
	cut: VideoCut | null;
	/** Inclusive years. A decade range from the river is [1980, 1999]. */
	eraFrom: number | null;
	eraTo: number | null;
	depth: ArchiveDepth | null;
	length: ArchiveLength | null;
	source: ArchiveSource;
	sort: ArchiveSort;
	q: string;
}

export const DEFAULT_LENSES: ArchiveLenses = Object.freeze({
	scene: null,
	cut: null,
	eraFrom: null,
	eraTo: null,
	depth: null,
	length: null,
	source: 'catalog',
	sort: 'relevance',
	q: '',
}) as ArchiveLenses;

export const CUTS: readonly VideoCut[] = [
	'official',
	'live',
	'session',
	'lyric',
	'visualizer',
	'remix',
	'dance',
	'longform',
	'clip',
	'interview',
	'trailer',
];

export const CUT_LABELS: Record<VideoCut, string> = {
	official: 'Official',
	live: 'Live',
	session: 'Session',
	lyric: 'Lyric',
	visualizer: 'Visualizer',
	remix: 'Remix',
	dance: 'Dance',
	interview: 'Interview',
	trailer: 'Trailer',
	clip: 'Clip',
	longform: 'Long form',
};

export const DEPTHS: readonly { value: ArchiveDepth; label: string }[] = [
	{ value: 'big', label: 'Big ones' },
	{ value: 'mid', label: 'Middle' },
	{ value: 'deep', label: 'Deep cuts' },
];

export const LENGTHS: readonly { value: ArchiveLength; label: string }[] = [
	{ value: 'clip', label: 'Clips' },
	{ value: 'song', label: 'Songs' },
	{ value: 'long', label: 'Long form' },
];

export const SOURCES: readonly { value: ArchiveSource; label: string }[] = [
	{ value: 'catalog', label: 'Catalog' },
	{ value: 'liked', label: 'Liked' },
	{ value: 'saved', label: 'Saved' },
];

export const SORTS: readonly { value: ArchiveSort; label: string }[] = [
	{ value: 'relevance', label: 'Best first' },
	{ value: 'popular', label: 'Most popular' },
	{ value: 'versions', label: 'Most versions' },
	{ value: 'year', label: 'Song year' },
	{ value: 'newest', label: 'Newest uploads' },
];

const MIN_YEAR = 1900;
const MAX_YEAR = 2100;

function oneOf<T extends string>(allowed: readonly T[], raw: string | null): T | null {
	return raw != null && (allowed as readonly string[]).includes(raw) ? (raw as T) : null;
}

function intIn(raw: string | null, min: number, max: number): number | null {
	if (raw == null || raw.trim() === '') return null;
	const value = Number(raw);
	return Number.isInteger(value) && value >= min && value <= max ? value : null;
}

/** Lenses from a query string. Anything malformed falls back to the default,
 *  so a stale or hand-edited link still opens the Archive. */
export function parseLenses(params: URLSearchParams): ArchiveLenses {
	let eraFrom = intIn(params.get('era_from'), MIN_YEAR, MAX_YEAR);
	let eraTo = intIn(params.get('era_to'), MIN_YEAR, MAX_YEAR);
	if (eraFrom != null && eraTo != null && eraFrom > eraTo) [eraFrom, eraTo] = [eraTo, eraFrom];
	return {
		scene: intIn(params.get('scene'), 0, Number.MAX_SAFE_INTEGER),
		cut: oneOf(CUTS, params.get('cut')),
		eraFrom,
		eraTo,
		depth: oneOf(DEPTHS.map((d) => d.value), params.get('depth')),
		length: oneOf(LENGTHS.map((l) => l.value), params.get('length')),
		source: oneOf(SOURCES.map((s) => s.value), params.get('source')) ?? DEFAULT_LENSES.source,
		sort: oneOf(SORTS.map((s) => s.value), params.get('sort')) ?? DEFAULT_LENSES.sort,
		q: (params.get('q') ?? '').trim(),
	};
}

/** The query string for a lens state, defaults omitted and keys in a fixed
 *  order, so equal lenses always give the same URL. Empty string or "?...". */
export function lensesToSearch(lenses: ArchiveLenses): string {
	const params = new URLSearchParams();
	if (lenses.scene != null) params.set('scene', String(lenses.scene));
	if (lenses.cut) params.set('cut', lenses.cut);
	if (lenses.eraFrom != null) params.set('era_from', String(lenses.eraFrom));
	if (lenses.eraTo != null) params.set('era_to', String(lenses.eraTo));
	if (lenses.depth) params.set('depth', lenses.depth);
	if (lenses.length) params.set('length', lenses.length);
	if (lenses.source !== DEFAULT_LENSES.source) params.set('source', lenses.source);
	if (lenses.sort !== DEFAULT_LENSES.sort) params.set('sort', lenses.sort);
	if (lenses.q.trim()) params.set('q', lenses.q.trim());
	const search = params.toString();
	return search ? `?${search}` : '';
}

/** The API query for one page of results under these lenses. */
export function lensesToQuery(lenses: ArchiveLenses, offset = 0, limit = 60): ArchiveQuery {
	const query: ArchiveQuery = { sort: lenses.sort, offset, limit };
	if (lenses.scene != null) query.scene = lenses.scene;
	if (lenses.cut) query.cut = lenses.cut;
	if (lenses.eraFrom != null) query.era_from = lenses.eraFrom;
	if (lenses.eraTo != null) query.era_to = lenses.eraTo;
	if (lenses.depth) query.depth = lenses.depth;
	if (lenses.length) query.length = lenses.length;
	if (lenses.source !== DEFAULT_LENSES.source) query.source = lenses.source;
	if (lenses.q.trim()) query.q = lenses.q.trim();
	return query;
}

/** Stable identity of what a lens state asks for (paging aside). */
export function lensKey(lenses: ArchiveLenses): string {
	return lensesToSearch(lenses);
}

/** Lenses that narrow the results (sort and source=catalog do not). */
export function activeLensCount(lenses: ArchiveLenses): number {
	let count = 0;
	if (lenses.scene != null) count += 1;
	if (lenses.cut) count += 1;
	if (lenses.eraFrom != null || lenses.eraTo != null) count += 1;
	if (lenses.depth) count += 1;
	if (lenses.length) count += 1;
	if (lenses.source !== DEFAULT_LENSES.source) count += 1;
	if (lenses.q.trim()) count += 1;
	return count;
}

/** Every narrowing lens off; the sort is a preference and stays. */
export function clearLenses(lenses: ArchiveLenses): ArchiveLenses {
	return { ...DEFAULT_LENSES, sort: lenses.sort };
}

/** Pick a value, or put it down again when it is already picked. */
export function toggleLens<K extends 'scene' | 'cut' | 'depth' | 'length'>(
	lenses: ArchiveLenses,
	key: K,
	value: NonNullable<ArchiveLenses[K]>
): ArchiveLenses {
	return { ...lenses, [key]: lenses[key] === value ? null : value };
}

/** The era as a short label: "1980s", "1980s - 1990s", "1994 - 2003". */
export function eraLabel(from: number | null, to: number | null): string {
	if (from == null && to == null) return 'Any year';
	if (from != null && to == null) return `${from} on`;
	if (from == null && to != null) return `Up to ${to}`;
	const a = from as number;
	const b = to as number;
	const decadeAligned = a % 10 === 0 && b % 10 === 9;
	if (decadeAligned) return a + 9 === b ? `${a}s` : `${a}s - ${b - 9}s`;
	return a === b ? String(a) : `${a} - ${b}`;
}

/** A short description of the lens state for the player's source label. */
export function lensSummary(lenses: ArchiveLenses, sceneName: string | null): string {
	const parts: string[] = [];
	if (lenses.scene != null) parts.push(sceneName ?? 'Scene');
	if (lenses.cut) parts.push(CUT_LABELS[lenses.cut]);
	if (lenses.eraFrom != null || lenses.eraTo != null) parts.push(eraLabel(lenses.eraFrom, lenses.eraTo));
	if (lenses.depth) parts.push(DEPTHS.find((d) => d.value === lenses.depth)?.label ?? '');
	if (lenses.length) parts.push(LENGTHS.find((l) => l.value === lenses.length)?.label ?? '');
	if (lenses.source !== 'catalog') parts.push(SOURCES.find((s) => s.value === lenses.source)?.label ?? '');
	if (lenses.q.trim()) parts.push(`"${lenses.q.trim()}"`);
	return parts.length ? `Archive: ${parts.filter(Boolean).join(', ')}` : 'Archive';
}

/** Compact counts for chips: 840, 12.4k, 1.2M. */
export function chipCount(value: number): string {
	if (value >= 1_000_000) return `${trim1(value / 1_000_000)}M`;
	if (value >= 10_000) return `${Math.round(value / 1000)}k`;
	if (value >= 1_000) return `${trim1(value / 1000)}k`;
	return String(value);
}

function trim1(value: number): string {
	return value.toFixed(1).replace(/\.0$/, '');
}

/** A scene's hue as a colour that reads on dark and light grounds. */
export function hueColor(hue: number, alpha = 1): string {
	const h = ((Math.round(hue) % 360) + 360) % 360;
	return alpha >= 1 ? `hsl(${h} 62% 56%)` : `hsl(${h} 62% 56% / ${alpha})`;
}
