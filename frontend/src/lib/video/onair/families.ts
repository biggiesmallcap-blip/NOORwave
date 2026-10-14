import type { VideoChannel } from '$lib/api/video_explore_types';
import { byNumber } from './lineup';

// The dial's scene channels in families (the server says which, from each
// scene's genre tags). The order is fixed, families as listed and channels by
// number inside each, so a channel is always where you left it: what you
// watch most already leads in the "for you" strip.

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

export interface DialFamily<T> {
	key: string;
	label: string;
	/** Numbered channels by number, then the locked "coming up" ones. */
	channels: T[];
}

type Groupable = Pick<VideoChannel, 'no' | 'family'>;

/** The fixed channels in number order, and the scene families in their
 *  fixed order. */
export function dialFamilies<T extends Groupable>(channels: readonly T[]): { main: T[]; families: DialFamily<T>[] } {
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
	const families = FAMILY_ORDER.filter((key) => groups.has(key)).map((key) => ({
		key,
		label: FAMILY_LABELS[key],
		channels: groups.get(key)!,
	}));
	return { main, families };
}
