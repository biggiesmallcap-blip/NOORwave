import { get, readable, writable } from 'svelte/store';
import { api } from '$lib/api/client';
import type { ExploreMode, VideoChannel, VideoChannelsResponse } from '$lib/api/video_explore_types';

// The day's lineup (every channel with what is on now and next), shared by
// the dial at /videos and the stage (flips, the guide). Kept across visits so
// the dial renders at once and refreshes quietly.

export interface Lineup {
	data: VideoChannelsResponse;
	/** Date.now() when the response landed: elapsed time moves the clock on. */
	fetchedAt: number;
}

const lineupStore = writable<Lineup | null>(null);
export const channelLineup = { subscribe: lineupStore.subscribe };

const exploreStore = writable<ExploreMode>('mixed');
/** Familiar / Mixed / Adventurous: how far from your taste the channels roam. */
export const exploreMode = { subscribe: exploreStore.subscribe };

/** Wall time, once a second, while anything on screen shows the clock. */
export const airTick = readable(Date.now(), (set) => {
	const timer = setInterval(() => set(Date.now()), 1000);
	return () => clearInterval(timer);
});

let inflight: Promise<Lineup> | null = null;

/** Fetch the lineup (one request at a time). */
export function loadLineup(): Promise<Lineup> {
	if (inflight) return inflight;
	const request = api.getVideoChannels().then((data) => {
		const lineup = { data, fetchedAt: Date.now() };
		lineupStore.set(lineup);
		if (data.explore) exploreStore.set(data.explore);
		return lineup;
	});
	inflight = request;
	void request.finally(() => {
		if (inflight === request) inflight = null;
	}).catch(() => {});
	return request;
}

/** The lineup if it is fresh enough, else a fetch. Null when it cannot load. */
export async function ensureLineup(maxAgeMs = 120_000): Promise<Lineup | null> {
	const current = get(lineupStore);
	if (current && Date.now() - current.fetchedAt < maxAgeMs && current.data.channels.length > 0) return current;
	try {
		return await loadLineup();
	} catch {
		return current;
	}
}

export function setLineupForTest(lineup: Lineup | null) {
	lineupStore.set(lineup);
}

/** Change the explore mode: shown at once, saved on the server, rolled back
 *  if the save fails. Resolves true when it stuck. */
export async function setExploreMode(mode: ExploreMode): Promise<boolean> {
	const previous = get(exploreStore);
	if (previous === mode) return true;
	exploreStore.set(mode);
	try {
		const saved = await api.putVideoExplore(mode);
		exploreStore.set(saved.explore ?? mode);
		return true;
	} catch {
		exploreStore.set(previous);
		return false;
	}
}

/** Read the saved explore mode (the lineup carries it too). */
export async function loadExploreMode(): Promise<void> {
	try {
		const { explore } = await api.getVideoExplore();
		if (explore) exploreStore.set(explore);
	} catch {
		// The lineup's own explore field covers it.
	}
}

/** Every channel in dial order. Unnumbered "coming up" scenes (no 0) go
 *  last, in the order the server ranked them. */
export function byNumber<T extends Pick<VideoChannel, 'no'>>(channels: readonly T[]): T[] {
	const key = (no: number) => (no > 0 ? no : Number.MAX_SAFE_INTEGER);
	return [...channels].sort((a, b) => key(a.no) - key(b.no));
}

/** "For you tonight": Your Channel, then the scene channels you have the
 *  most appetite for. The server sends channels in appetite order, so its
 *  order is the ranking; fixed channels fill in when there are few scenes.
 *  Only channels with something on now. */
export function forYouTonight(channels: readonly VideoChannel[], size = 4): VideoChannel[] {
	const live = channels.filter((channel) => !channel.locked && channel.now);
	const picks: VideoChannel[] = [];
	const your = live.find((channel) => channel.id === 'your');
	if (your) picks.push(your);
	for (const channel of live) {
		if (picks.length >= size) break;
		if (channel.scene_id != null && !picks.includes(channel)) picks.push(channel);
	}
	for (const channel of live) {
		if (picks.length >= Math.min(size, 3)) break;
		if (!picks.includes(channel)) picks.push(channel);
	}
	return picks;
}

// Fixed channels have no scene hue; each gets a stable one of its own so the
// dial and the stage colour-code every channel the same way.
const FIXED_HUES: Record<string, number> = {
	your: 350,
	wild: 295,
	premieres: 85,
	live: 25,
	sessions: 150,
	concerts: 262,
	rewind: 55,
	spotlight: 200,
};

/** The channel's hue in degrees (scene hue, or the fixed channel's own). */
export function channelHue(channel: Pick<VideoChannel, 'id' | 'hue' | 'no'>): number {
	if (channel.hue != null && Number.isFinite(channel.hue)) return ((channel.hue % 360) + 360) % 360;
	return FIXED_HUES[channel.id] ?? (channel.no * 47) % 360;
}
