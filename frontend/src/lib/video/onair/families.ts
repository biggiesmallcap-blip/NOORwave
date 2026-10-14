import type { VideoChannel } from '$lib/api/video_explore_types';
import { byNumber } from './lineup';

// The dial in families: the fixed channels first, then the scene channels
// under a few named families (the server says which), the families you watch
// most first. A family is as warm as its three warmest scenes. Families well
// below your warmest fold to one line until opened; a family you opened or
// folded yourself stays that way.

export const FAMILY_LABELS: Record<string, string> = {
	hiphop: 'Hip Hop and R&B',
	latin: 'Latin and Caribbean',
	rock: 'Rock and Alternative',
	pop: 'Pop and Dance',
	country: 'Country and Folk',
	soul: 'Soul, Jazz and Classical',
	world: 'Around the World',
	other: 'More scenes',
};
const FAMILY_ORDER = Object.keys(FAMILY_LABELS);

/** A family is judged by this many of its warmest scenes. */
const FAMILY_TOP = 3;
/** A family below this share of the warmest family's appetite starts folded.
 *  Relative, so a dial with no history yet (all alike) folds nothing. */
export const FOLD_SHARE = 0.5;

export interface DialFamily<T> {
	key: string;
	label: string;
	channels: T[];
	/** The mean appetite of its warmest scenes, 0 when none is known. */
	appetite: number;
	/** Folded unless you opened it, or it holds the channel on air. */
	folded: boolean;
}

type Groupable = Pick<VideoChannel, 'id' | 'no' | 'family' | 'appetite'>;

/** The fixed channels in number order, then the families. `choices` are the
 *  viewer's own open (false) or fold (true) picks by family key. */
export function dialFamilies<T extends Groupable>(
	channels: readonly T[],
	onAirId: string | null = null,
	choices: Readonly<Record<string, boolean>> = {},
): { main: T[]; families: DialFamily<T>[] } {
	const main: T[] = [];
	const groups = new Map<string, T[]>();
	for (const channel of byNumber(channels)) {
		if (channel.family == null) {
			main.push(channel);
			continue;
		}
		const key = channel.family in FAMILY_LABELS ? channel.family : 'other';
		const group = groups.get(key);
		if (group) group.push(channel);
		else groups.set(key, [channel]);
	}
	const warmth = (members: T[]) => {
		const top = members
			.map((channel) => channel.appetite ?? 0)
			.sort((a, b) => b - a)
			.slice(0, FAMILY_TOP);
		return top.reduce((sum, value) => sum + value, 0) / top.length;
	};
	const scored = [...groups.entries()].map(([key, members]) => ({ key, members, appetite: warmth(members) }));
	const warmest = Math.max(0, ...scored.map((family) => family.appetite));
	const families = scored.map(({ key, members, appetite }) => {
		const holdsOnAir = onAirId != null && members.some((channel) => channel.id === onAirId);
		const folded = holdsOnAir ? false : (choices[key] ?? appetite < warmest * FOLD_SHARE);
		return { key, label: FAMILY_LABELS[key], channels: members, appetite, folded };
	});
	families.sort((a, b) => b.appetite - a.appetite || FAMILY_ORDER.indexOf(a.key) - FAMILY_ORDER.indexOf(b.key));
	return { main, families };
}

const CHOICES_KEY = 'noor.onair.families.v1';

/** The viewer's open and fold picks; empty when storage is unavailable. */
export function loadFamilyChoices(): Record<string, boolean> {
	try {
		const raw = localStorage.getItem(CHOICES_KEY);
		const parsed: unknown = raw ? JSON.parse(raw) : null;
		if (!parsed || typeof parsed !== 'object') return {};
		return Object.fromEntries(Object.entries(parsed).filter(([, value]) => typeof value === 'boolean')) as Record<string, boolean>;
	} catch {
		return {};
	}
}

export function saveFamilyChoices(choices: Record<string, boolean>): void {
	try {
		localStorage.setItem(CHOICES_KEY, JSON.stringify(choices));
	} catch {
		// A full or blocked storage just forgets the pick next visit.
	}
}
