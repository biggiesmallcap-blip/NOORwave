/**
 * Blind radio comparison: the session the /radio-compare page edits and saves.
 *
 * Each seed holds the station designs' lists (from POST /api/radio/compare),
 * a shuffled display order that hides which design is which, the tracks the
 * listener marked as not fitting, and the station they picked as best. All
 * updates return a new session so the page can hold it in `$state`.
 */
import type { RadioCompareArm, RadioCompareTrack } from '$lib/api/client';

export const COMPARE_SESSION_VERSION = 1;

export interface CompareSeedEntry {
	seed: RadioCompareTrack;
	/** `null` until the lists are loaded. */
	arms: RadioCompareArm[] | null;
	/** Arm ids in display order: the first is station A. */
	order: string[];
	/** Arm id to the keys of tracks marked as not fitting the seed. */
	misfits: Record<string, string[]>;
	/** The arm picked as best; a seed counts in the results once this is set. */
	best: string | null;
	/** The arm picked as worst, if any. Never the same as `best`. */
	worst: string | null;
}

export interface CompareSession {
	version: typeof COMPARE_SESSION_VERSION;
	entries: CompareSeedEntry[];
}

export interface ArmSummary {
	arm: string;
	/** Judged seeds this design had a list for. */
	seeds: number;
	tracks: number;
	misfits: number;
	/** Share of shown tracks not marked; `null` when it showed none. */
	fit: number | null;
	wins: number;
	/** Judged seeds where this design was picked as worst. */
	losses: number;
	/** Judged seeds where this design had nothing to play. */
	empty: number;
}

const ARM_NAMES: Record<string, string> = {
	current: 'Current station',
	lastfm: 'Last.fm only',
	tidal: 'TIDAL radio',
	hybrid: 'Hybrid (TIDAL + Last.fm core)',
};

/** Results list designs in this order; unknown designs follow. */
const ARM_ORDER = ['current', 'lastfm', 'tidal', 'hybrid'];

export function armName(arm: string): string {
	return ARM_NAMES[arm] ?? arm;
}

export function stationLabel(index: number): string {
	return String.fromCharCode(65 + index);
}

export function emptySession(): CompareSession {
	return { version: COMPARE_SESSION_VERSION, entries: [] };
}

/** Stable identity for a row: TIDAL id, or the library id for a local file. */
export function trackKey(track: Pick<RadioCompareTrack, 'tidal_id' | 'local_id'>): string {
	return track.tidal_id > 0 ? `t${track.tidal_id}` : `l${track.local_id ?? 0}`;
}

/** Fisher-Yates over a copy. */
export function shuffled<T>(items: readonly T[], random: () => number = Math.random): T[] {
	const out = [...items];
	for (let i = out.length - 1; i > 0; i--) {
		const j = Math.floor(random() * (i + 1));
		[out[i], out[j]] = [out[j], out[i]];
	}
	return out;
}

function seedId(entry: CompareSeedEntry): number | null {
	return entry.seed.local_id;
}

function updateEntry(
	session: CompareSession,
	localId: number,
	update: (entry: CompareSeedEntry) => CompareSeedEntry
): CompareSession {
	return {
		...session,
		entries: session.entries.map((entry) => (seedId(entry) === localId ? update(entry) : entry)),
	};
}

export function addSeed(session: CompareSession, seed: RadioCompareTrack): CompareSession {
	if (seed.local_id == null || session.entries.some((entry) => seedId(entry) === seed.local_id)) {
		return session;
	}
	const entry: CompareSeedEntry = {
		seed,
		arms: null,
		order: [],
		misfits: {},
		best: null,
		worst: null,
	};
	return { ...session, entries: [...session.entries, entry] };
}

export function removeSeed(session: CompareSession, localId: number): CompareSession {
	return { ...session, entries: session.entries.filter((entry) => seedId(entry) !== localId) };
}

/**
 * Store a seed's lists. The display order is shuffled the first time and kept
 * while the same designs come back, so a reload neither reshuffles nor loses
 * marks.
 */
export function withArms(
	session: CompareSession,
	localId: number,
	arms: RadioCompareArm[],
	random: () => number = Math.random
): CompareSession {
	return updateEntry(session, localId, (entry) => {
		const ids = arms.map((a) => a.arm);
		const sameDesigns =
			entry.order.length === ids.length && ids.every((id) => entry.order.includes(id));
		const order = sameDesigns ? entry.order : shuffled(ids, random);
		const misfits = Object.fromEntries(
			Object.entries(entry.misfits).filter(([arm]) => ids.includes(arm))
		);
		const best = entry.best && ids.includes(entry.best) ? entry.best : null;
		const worst = entry.worst && ids.includes(entry.worst) ? entry.worst : null;
		return { ...entry, arms, order, misfits, best, worst };
	});
}

export function toggleMisfit(
	session: CompareSession,
	localId: number,
	arm: string,
	key: string
): CompareSession {
	return updateEntry(session, localId, (entry) => {
		const marked = entry.misfits[arm] ?? [];
		const next = marked.includes(key) ? marked.filter((k) => k !== key) : [...marked, key];
		return { ...entry, misfits: { ...entry.misfits, [arm]: next } };
	});
}

/** Pick the best station; picking it again clears the pick. */
export function setBest(session: CompareSession, localId: number, arm: string | null): CompareSession {
	return updateEntry(session, localId, (entry) => {
		const best = entry.best === arm ? null : arm;
		return { ...entry, best, worst: best !== null && entry.worst === best ? null : entry.worst };
	});
}

/** Pick the worst station; picking it again clears the pick. */
export function setWorst(session: CompareSession, localId: number, arm: string | null): CompareSession {
	return updateEntry(session, localId, (entry) => {
		const worst = entry.worst === arm ? null : arm;
		return { ...entry, worst, best: worst !== null && entry.best === worst ? null : entry.best };
	});
}

export function summarize(session: CompareSession): ArmSummary[] {
	const byArm = new Map<string, ArmSummary>();
	for (const entry of session.entries) {
		if (!entry.best || !entry.arms) continue;
		for (const list of entry.arms) {
			const summary = byArm.get(list.arm) ?? {
				arm: list.arm,
				seeds: 0,
				tracks: 0,
				misfits: 0,
				fit: null,
				wins: 0,
				losses: 0,
				empty: 0,
			};
			const keys = new Set(list.tracks.map(trackKey));
			summary.seeds += 1;
			summary.tracks += list.tracks.length;
			summary.misfits += (entry.misfits[list.arm] ?? []).filter((k) => keys.has(k)).length;
			if (list.tracks.length === 0) summary.empty += 1;
			if (entry.best === list.arm) summary.wins += 1;
			if (entry.worst === list.arm) summary.losses += 1;
			byArm.set(list.arm, summary);
		}
	}
	const rank = (arm: string) => {
		const i = ARM_ORDER.indexOf(arm);
		return i === -1 ? ARM_ORDER.length : i;
	};
	return [...byArm.values()]
		.map((s) => ({ ...s, fit: s.tracks > 0 ? 1 - s.misfits / s.tracks : null }))
		.sort((a, b) => rank(a.arm) - rank(b.arm) || a.arm.localeCompare(b.arm));
}

function isEntry(value: unknown): value is CompareSeedEntry {
	if (typeof value !== 'object' || value === null) return false;
	const entry = value as Record<string, unknown>;
	const seed = entry.seed as Record<string, unknown> | undefined;
	return (
		typeof seed === 'object' &&
		seed !== null &&
		typeof seed.local_id === 'number' &&
		(entry.arms === null || Array.isArray(entry.arms)) &&
		Array.isArray(entry.order) &&
		typeof entry.misfits === 'object' &&
		entry.misfits !== null &&
		(entry.best === null || typeof entry.best === 'string') &&
		(entry.worst === undefined || entry.worst === null || typeof entry.worst === 'string')
	);
}

/** A saved session, or an empty one for anything that is not one. */
export function parseSession(raw: unknown): CompareSession {
	if (typeof raw !== 'object' || raw === null) return emptySession();
	const session = raw as Record<string, unknown>;
	if (session.version !== COMPARE_SESSION_VERSION || !Array.isArray(session.entries)) {
		return emptySession();
	}
	if (!session.entries.every(isEntry)) return emptySession();
	return {
		version: COMPARE_SESSION_VERSION,
		// Sessions saved before worst picks existed have no `worst`.
		entries: (session.entries as CompareSeedEntry[]).map((entry) => ({
			...entry,
			worst: entry.worst ?? null,
		})),
	};
}
