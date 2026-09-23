import { derived, get, writable } from 'svelte/store';
import { api } from '$lib/api/client';
import type { TidalSearchVideo, TidalVideoMixItem } from '$lib/api/client';

export type VideoSessionItem = TidalSearchVideo | TidalVideoMixItem;
export type VideoSessionSource = 'none' | 'direct' | 'search' | 'mix';

export interface VideoSessionState {
	active: boolean;
	current: VideoSessionItem | null;
	queue: VideoSessionItem[];
	currentIndex: number;
	source: VideoSessionSource;
	sourceLabel: string | null;
	autoplay: boolean;
	continuous: boolean;
	loading: boolean;
	error: string | null;
	/** HLS stream URL for `current`. Lives in the store so the persistent dock
	 *  can keep playing across route changes without the route owning it. */
	streamUrl: string | null;
	streamExpiresAt: string | null;
	playing: boolean;
	/** Last reported playback position, kept so returning to /videos resumes
	 *  the picture in sync with the audio that never stopped. */
	positionMs: number;
}

/** Browse context the route hands to the controller when it starts a video:
 *  the queue to autoplay through and how it was sourced. */
export interface VideoPlayContext {
	queue: VideoSessionItem[];
	source: VideoSessionSource;
	sourceLabel: string | null;
	autoplay?: boolean;
	continuous?: boolean;
	resetRadio?: boolean;
}

export interface PreloadedVideoStream {
	url: string;
	expiresAt: string | null;
}

const AUTOPLAY_KEY = 'noor_video_autoplay_next';

function loadAutoplayPreference(): boolean {
	if (typeof localStorage === 'undefined') return false;
	return localStorage.getItem(AUTOPLAY_KEY) === 'true';
}

function persistAutoplayPreference(autoplay: boolean) {
	if (typeof localStorage === 'undefined') return;
	localStorage.setItem(AUTOPLAY_KEY, String(autoplay));
}

const initialState: VideoSessionState = {
	active: false,
	current: null,
	queue: [],
	currentIndex: -1,
	source: 'none',
	sourceLabel: null,
	autoplay: loadAutoplayPreference(),
	continuous: false,
	loading: false,
	error: null,
	streamUrl: null,
	streamExpiresAt: null,
	playing: false,
	positionMs: 0,
};

function findCurrentIndex(queue: VideoSessionItem[], current: VideoSessionItem | null): number {
	if (!current) return -1;
	return queue.findIndex((item) => item.tidal_id === current.tidal_id);
}

const session = writable<VideoSessionState>(initialState);

function update(patch: Partial<VideoSessionState>) {
	session.update((state) => {
		const next = { ...state, ...patch };
		next.currentIndex = findCurrentIndex(next.queue, next.current);
		next.active = next.current !== null;
		return next;
	});
}

export const videoSession = {
	subscribe: session.subscribe,
	/** Refresh the autoplay queue + source attribution without touching the
	 *  currently-playing video (called as the route's search results change). */
	setContext(ctx: { queue: VideoSessionItem[]; source: VideoSessionSource; sourceLabel: string | null }) {
		update({ queue: ctx.queue, source: ctx.source, sourceLabel: ctx.sourceLabel });
	},
	setAutoplay(autoplay: boolean) {
		persistAutoplayPreference(autoplay);
		update({ autoplay });
	},
	setPlaying(playing: boolean) {
		update({ playing });
	},
	setPosition(positionMs: number) {
		update({ positionMs });
	},
	reset() {
		session.set({ ...initialState, autoplay: loadAutoplayPreference() });
	},
};

export const videoSessionUpcoming = derived(session, ($session) => {
	if ($session.currentIndex < 0) return $session.queue;
	return $session.queue.slice($session.currentIndex + 1);
});

// ─── Controller: owns the stream lifecycle so playback survives navigation ───

let streamSeq = 0;
let radioGeneration = 0;
let radioRefill: Promise<number> | null = null;
let radioSeenIds: number[] = [];

function sourceFor(item: VideoSessionItem, ctx: VideoPlayContext): VideoSessionSource {
	if (ctx.source !== 'none') return ctx.source;
	if ('mix_id' in item && item.mix_id != null) return 'mix';
	return 'direct';
}

/** Log a started video so the editorial builder can hold it out of the next few
 *  rotations. Fire-and-forget: a dropped write only costs a repeat pick. */
function recordWatch(item: VideoSessionItem) {
	const artistId = 'artist_id' in item ? item.artist_id : null;
	void api
		.recordVideoHistory({
			tidal_video_id: item.tidal_id,
			title: item.title ?? null,
			artist_tidal_id: artistId ?? null,
			artist_name: item.artist_name ?? null,
		})
		.catch(() => {});
}

/** Start (or switch to) a video: set it current, fetch its HLS stream, and let
 *  the persistent dock render it. Returns false if the request was superseded
 *  or the stream failed. */
export async function playVideo(
	item: VideoSessionItem,
	ctx: VideoPlayContext,
	opts: { preloaded?: PreloadedVideoStream | null } = {}
): Promise<boolean> {
	// Re-selecting the video that is already playing (returning to /videos with
	// its ?videoId= still in the URL, a stale jump request, clicking its own
	// queue row) must not refetch the stream and restart it from 0:00. Refresh
	// the browse context and show it, but leave playback untouched.
	const state = get(session);
	if (ctx.resetRadio) {
		radioGeneration += 1;
		radioSeenIds = [item.tidal_id];
		radioRefill = null;
	}
	if (
		state.active &&
		state.current?.tidal_id === item.tidal_id &&
		Boolean(state.streamUrl) &&
		!state.error &&
		!opts.preloaded
	) {
		videoBrowseMode.set(false);
		update({
			queue: ctx.queue,
			source: sourceFor(item, ctx),
			sourceLabel: ctx.sourceLabel,
			autoplay: ctx.autoplay ?? state.autoplay,
			continuous: ctx.continuous ?? false,
		});
		return true;
	}

	const seq = ++streamSeq;
	// Picking something new always means "show it": browsing the shelves with
	// a video docked ends the moment you choose the next one.
	videoBrowseMode.set(false);
	update({
		current: item,
		queue: ctx.queue,
		source: sourceFor(item, ctx),
		sourceLabel: ctx.sourceLabel,
		autoplay: ctx.autoplay ?? get(session).autoplay,
		continuous: ctx.continuous ?? false,
		loading: true,
		error: null,
		streamUrl: opts.preloaded?.url ?? null,
		streamExpiresAt: opts.preloaded?.expiresAt ?? null,
		positionMs: 0,
	});

	try {
		let url = opts.preloaded?.url ?? null;
		let expiresAt = opts.preloaded?.expiresAt ?? null;
		if (!url) {
			const stream = await api.getTidalVideoStream(item.tidal_id);
			if (seq !== streamSeq) return false;
			url = stream.hls_url;
			expiresAt = stream.expires_at;
		}
		if (seq !== streamSeq) return false;
		update({ streamUrl: url, streamExpiresAt: expiresAt, loading: false, error: null });
		if (ctx.continuous) radioSeenIds.push(item.tidal_id);
		recordWatch(item);
		if (ctx.continuous) void refillVideoRadio(Boolean(ctx.resetRadio));
		return true;
	} catch (err) {
		if (seq !== streamSeq) return false;
		const message = err instanceof Error ? err.message : 'This video could not be loaded.';
		update({ loading: false, error: message });
		return false;
	}
}

/** Keep a short lookahead in the persistent dock's session. A single request
 * can be shared by the prefetch effect and the end-of-video path. */
export function refillVideoRadio(force = false): Promise<number> {
	if (radioRefill) return radioRefill;
	const state = get(session);
	if (!state.active || !state.continuous || !state.autoplay || (!force && state.queue.length - state.currentIndex > 5)) {
		return Promise.resolve(0);
	}
	const generation = radioGeneration;
	const seed = state.current;
	const excluded = state.queue.map((v) => v.tidal_id);
	const recentArtists = state.queue.slice(Math.max(0, state.currentIndex - 8), state.currentIndex + 1)
		.map((v) => v.artist_id).filter((id): id is number => id != null);
	const pending = api.getVideoRadioNext({
		seed_artist_id: seed?.artist_id ?? null,
		seed_artist_name: seed?.artist_name ?? null,
		exclude_video_ids: excluded,
		recent_video_ids: radioSeenIds.slice(-96),
		recent_artist_ids: recentArtists,
	}).then(({ items, unfamiliar_video_ids }) => {
		const current = get(session);
		if (generation !== radioGeneration || !current.continuous || !current.active) return 0;
		const existing = new Set(current.queue.map((v) => v.tidal_id));
		const fresh = items.filter((item) => !existing.has(item.tidal_id));
		if (fresh.length === 0) return 0;
		if (force && fresh.length >= 8 && unfamiliar_video_ids.filter((id) => fresh.some((item) => item.tidal_id === id)).length >= 2) {
			// The browse shelves are a quick start. Once the server has a real
			// discovery blend, hand upcoming playback to it immediately.
			update({ queue: [...current.queue.slice(0, current.currentIndex + 1), ...fresh] });
		} else if (!force) {
			const start = Math.max(0, current.currentIndex - 6);
			update({ queue: [...current.queue.slice(start), ...fresh] });
		} else {
			return 0;
		}
		radioSeenIds.push(...fresh.map((item) => item.tidal_id));
		radioSeenIds = radioSeenIds.slice(-256);
		return fresh.length;
	}).catch(() => 0);
	radioRefill = pending;
	void pending.finally(() => { if (radioRefill === pending) radioRefill = null; });
	return pending;
}

/** Re-fetch the current video's stream (expiry / network recovery). */
export async function refreshVideoStream(): Promise<string> {
	const current = get(session).current;
	if (!current) throw new Error('No video selected.');
	const seq = ++streamSeq;
	const stream = await api.getTidalVideoStream(current.tidal_id);
	if (seq !== streamSeq) throw Object.assign(new Error('Stream request superseded.'), { name: 'StaleStreamRequest' });
	update({ streamUrl: stream.hls_url, streamExpiresAt: stream.expires_at });
	return stream.hls_url;
}

/** Advance to the next queued video when autoplay is on. Returns false at the
 *  end of the loaded queue (the route tops the queue up while it's mounted). */
export async function advanceVideo(opts: { preloaded?: PreloadedVideoStream | null } = {}): Promise<boolean> {
	const state = get(session);
	if (!state.autoplay) return false;
	const index = findCurrentIndex(state.queue, state.current);
	if (index < 0) return false;
	const next = state.queue[index + 1];
	if (!next) {
		if (state.continuous) {
			await refillVideoRadio();
			const refreshed = get(session);
			// A click may have replaced radio while its network request was in
			// flight. Leave the new session and its autoplay preference alone.
			if (!refreshed.continuous || !refreshed.autoplay || refreshed.current?.tidal_id !== state.current?.tidal_id) return true;
			const nextIndex = findCurrentIndex(refreshed.queue, refreshed.current);
			const replenished = refreshed.queue[nextIndex + 1];
			if (replenished) return playVideo(replenished, {
				queue: refreshed.queue, source: refreshed.source,
				sourceLabel: refreshed.sourceLabel, autoplay: true, continuous: true,
			});
		}
		update({ playing: false });
		return false;
	}
	return playVideo(next, {
		queue: state.queue,
		source: state.source,
		sourceLabel: state.sourceLabel,
		autoplay: true,
		continuous: state.continuous,
	}, opts);
}

/** Stop the video session entirely and free the dock. */
export function clearVideoSession() {
	streamSeq += 1;
	radioGeneration += 1;
	radioSeenIds = [];
	radioRefill = null;
	session.set({ ...initialState, autoplay: loadAutoplayPreference() });
	videoBrowseMode.set(false);
}

// ─── Cross-component requests (dispatched from layout, served by the dock) ───

/** The /videos route's in-page placeholder. The persistent dock copies this
 *  element's rect each frame so the live player appears docked into the hero
 *  while actually being a fixed element that never unmounts on navigation. */
export const videoStageAnchor = writable<HTMLElement | null>(null);

/** True while the listener has stepped back to the picks with a video still
 *  playing. The route hides its stage anchor, so the dock falls to its mini
 *  corner player and the editorial shelves take the page back. Playback is
 *  untouched either way - this only decides who owns the hero slot. */
export const videoBrowseMode = writable(false);

export function setVideoBrowseMode(browsing: boolean) {
	videoBrowseMode.set(browsing);
}

export const videoJumpRequest = writable<{ videoId: number; nonce: number } | null>(null);
export const videoAutoplayToggleRequest = writable(0);
export const videoClearRequest = writable(0);

let jumpNonce = 0;

export function requestVideoJump(videoId: number) {
	videoJumpRequest.set({ videoId, nonce: ++jumpNonce });
}

export function requestVideoAutoplayToggle() {
	videoAutoplayToggleRequest.update((nonce) => nonce + 1);
}

export function requestVideoClear() {
	videoClearRequest.update((n) => n + 1);
}
