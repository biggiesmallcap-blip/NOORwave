import { derived, get, writable } from 'svelte/store';
import { api } from '$lib/api/client';
import type { TidalSearchVideo, TidalVideoMixItem } from '$lib/api/client';

export type VideoSessionItem = TidalSearchVideo | TidalVideoMixItem;
export type VideoSessionSource = 'none' | 'direct' | 'search' | 'mix';
export interface VideoRadioHit { artist: string; count: number }

export interface VideoSessionState {
	active: boolean;
	current: VideoSessionItem | null;
	queue: VideoSessionItem[];
	currentIndex: number;
	source: VideoSessionSource;
	sourceLabel: string | null;
	autoplay: boolean;
	continuous: boolean;
	radioSeedArtistId: number | null;
	radioSeedArtistName: string | null;
	loading: boolean;
	error: string | null;
	radioIssue: string | null;
	radioSearching: boolean;
	radioDiscoveryMessage: string | null;
	radioHits: VideoRadioHit[];
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
	/** Library radio keeps the supplied opening queue and refills from the
	 * listener's taste and recently played artists instead of one fixed artist. */
	radioScope?: 'artist' | 'library' | 'station';
	/** Station mode: refills come from this station instead of artist radio. */
	stationId?: string;
	stationNonce?: string;
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
	radioSeedArtistId: null,
	radioSeedArtistName: null,
	loading: false,
	error: null,
	radioIssue: null,
	radioSearching: false,
	radioDiscoveryMessage: null,
	radioHits: [],
	streamUrl: null,
	streamExpiresAt: null,
	playing: false,
	positionMs: 0,
};

function findCurrentIndex(queue: VideoSessionItem[], current: VideoSessionItem | null): number {
	if (!current) return -1;
	return queue.findIndex((item) => item.tidal_id === current.tidal_id);
}

function videoSongKey(item: Pick<VideoSessionItem, 'artist_id' | 'artist_name' | 'title'>): string {
	const artist = item.artist_id != null ? `id:${item.artist_id}` : `name:${item.artist_name?.trim().toLowerCase() ?? ''}`;
	const title = item.title.split(/[([]/, 1)[0].toLowerCase().replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
	return `${artist}:${title || item.title.toLowerCase()}`;
}

function radioStartQueue(item: VideoSessionItem): VideoSessionItem[] {
	// Artist radio starts from its seed alone. Library radio keeps its mixed
	// opening queue instead of using this path.
	return [item];
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
		if (autoplay && get(session).continuous) void refillVideoRadio(true);
	},
	startRadio() {
		const state = get(session);
		if (!state.current) return;
		const retryEndedVideo = Boolean(state.radioIssue);
		const currentVideoId = state.current.tidal_id;
		radioGeneration += 1;
		radioSeenIds = [state.current.tidal_id];
		radioSeenSongs = [state.current];
		radioSeedVideoId = state.current.tidal_id;
		radioRefill = null;
		clearStation();
		persistAutoplayPreference(true);
		const queue = radioStartQueue(state.current);
		update({ queue, continuous: true, autoplay: true, radioIssue: null,
			radioSearching: false, radioDiscoveryMessage: null, radioHits: [],
			radioSeedArtistId: state.current.artist_id ?? null, radioSeedArtistName: state.current.artist_name ?? null,
			sourceLabel: `${state.current.artist_name ?? 'Video'} radio` });
		void refillVideoRadio(true).then(async () => {
			if (!retryEndedVideo) return;
			const current = get(session);
			if (!current.continuous || current.current?.tidal_id !== currentVideoId) return;
			if (!current.queue[current.currentIndex + 1] || !(await advanceVideo())) {
				videoSession.radioExhausted(currentVideoId);
			}
		});
	},
	stopRadio() {
		radioGeneration += 1;
		radioRefill = null;
		radioSeedVideoId = null;
		clearStation();
		update({ continuous: false, radioSeedArtistId: null, radioSeedArtistName: null,
			radioIssue: null, radioSearching: false, radioDiscoveryMessage: null, radioHits: [], sourceLabel: 'Video queue' });
	},
	radioExhausted(expectedVideoId: number) {
		const state = get(session);
		if (!state.continuous || state.current?.tidal_id !== expectedVideoId) return;
		radioGeneration += 1;
		radioRefill = null;
		radioSeedVideoId = null;
		const issue = radioStationTitle
			? `You've seen everything in ${radioStationTitle}. Pick another station to keep going.`
			: 'Radio could not find another video. Start radio to try again.';
		clearStation();
		persistAutoplayPreference(false);
		update({ continuous: false, radioSeedArtistId: null, radioSeedArtistName: null,
			autoplay: false, playing: false, radioSearching: false, sourceLabel: 'Video queue',
			radioIssue: issue });
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

function queueWithCurrent(state: VideoSessionState): VideoSessionItem[] {
	// A direct/search selection may not be in its browse results. Give manual
	// queue actions a current-video anchor so Next and autoplay can advance.
	return state.current && findCurrentIndex(state.queue, state.current) < 0
		? [state.current, ...state.queue] : [...state.queue];
}

/** Append without loading a stream or interrupting the current video. */
export function addVideoToQueue(item: VideoSessionItem): boolean {
	const state = get(session);
	const queue = queueWithCurrent(state);
	if (queue.some(video => video.tidal_id === item.tidal_id)) return false;
	update({ queue: [...queue, item], ...(state.source === 'none' ? { source: 'direct', sourceLabel: 'Video queue' } : {}) });
	return true;
}

/** Insert or move a video directly after the current video. */
export function playVideoNext(item: VideoSessionItem): boolean {
	const state = get(session);
	if (state.current?.tidal_id === item.tidal_id) return false;
	const queued = state.queue.find(video => video.tidal_id === item.tidal_id);
	const queue = queueWithCurrent(state).filter(video => video.tidal_id !== item.tidal_id);
	queue.splice(findCurrentIndex(queue, state.current) + 1, 0, queued ?? item);
	update({ queue, ...(state.source === 'none' ? { source: 'direct', sourceLabel: 'Video queue' } : {}) });
	return true;
}

/** Remove only upcoming videos; the current stream remains untouched. */
export function removeVideoFromQueue(videoId: number): boolean {
	const state = get(session);
	const index = state.queue.findIndex(video => video.tidal_id === videoId);
	if (index <= state.currentIndex) return false;
	const removed = state.queue[index];
	if (!removed) return false;
	// An in-flight radio response must not put a deliberately removed cut back.
	radioSeenIds = [...radioSeenIds, videoId].slice(-256);
	radioSeenSongs = [...radioSeenSongs, removed].slice(-128);
	update({ queue: state.queue.filter(video => video.tidal_id !== videoId) });
	return true;
}

/** Queue rows use the persistent session even when /videos is not mounted. */
export async function playQueuedVideo(videoId: number): Promise<boolean> {
	const state = get(session);
	const item = state.queue.find(video => video.tidal_id === videoId);
	if (!item) return false;
	return playVideo(item, {
		queue: state.queue, source: state.source, sourceLabel: state.sourceLabel,
		autoplay: state.autoplay, continuous: state.continuous,
	});
}

// ─── Controller: owns the stream lifecycle so playback survives navigation ───

let streamSeq = 0;
let radioGeneration = 0;
let radioRefill: Promise<number> | null = null;
let radioSeenIds: number[] = [];
let radioSeenSongs: VideoSessionItem[] = [];
let radioSeedVideoId: number | null = null;
let radioStationId: string | null = null;
let radioStationTitle: string | null = null;
let radioStationNonce = '';

function clearStation() {
	radioStationId = null;
	radioStationTitle = null;
	radioStationNonce = '';
}
let lastRefillBuilding = false;

/** How a station that is still being built is waited on. Tests shorten it. */
export const radioRetryPolicy = { delayMs: 4000, attempts: 5 };

// --- Watch reporting: feeds the crawler's "enjoyed" roots ---

interface WatchRecord {
	videoId: number;
	historyId: number | null;
	watchedMs: number;
	durationMs: number | null;
	finished: boolean;
	completed: boolean;
}

let watch: WatchRecord | null = null;

function sendFinish(record: WatchRecord) {
	if (record.historyId == null) return;
	void api
		.finishVideoHistory(record.historyId, {
			watched_ms: Math.round(record.watchedMs),
			video_duration_ms: record.durationMs != null ? Math.round(record.durationMs) : null,
			completed: record.completed,
		})
		.catch(() => {});
}

function finishWatch(completed: boolean) {
	const record = watch;
	if (!record || record.finished) return;
	record.finished = true;
	record.completed = completed;
	sendFinish(record);
}

/** Position from the dock's player. Kept outside the store so a 4 Hz
 *  timeupdate does not re-render every subscriber. */
export function noteVideoProgress(positionMs: number, durationMs?: number) {
	if (!watch || watch.finished || !Number.isFinite(positionMs)) return;
	watch.watchedMs = Math.max(watch.watchedMs, positionMs);
	if (durationMs && Number.isFinite(durationMs) && durationMs > 0) watch.durationMs = durationMs;
}

/** The current video played to its end. */
export function reportVideoEnded() {
	finishWatch(true);
}

function sourceFor(item: VideoSessionItem, ctx: VideoPlayContext): VideoSessionSource {
	if (ctx.source !== 'none') return ctx.source;
	if ('mix_id' in item && item.mix_id != null) return 'mix';
	return 'direct';
}

/** Log a started video so the editorial builder can hold it out of the next few
 *  rotations, then report how much of it was watched. Fire-and-forget: a
 *  dropped write only costs a repeat pick. */
function recordWatch(item: VideoSessionItem) {
	if (watch && watch.videoId !== item.tidal_id) finishWatch(false);
	const record: WatchRecord = {
		videoId: item.tidal_id, historyId: null, watchedMs: 0,
		durationMs: item.duration_ms ?? null, finished: false, completed: false,
	};
	watch = record;
	const artistId = 'artist_id' in item ? item.artist_id : null;
	void api
		.recordVideoHistory({
			tidal_video_id: item.tidal_id,
			title: item.title ?? null,
			artist_tidal_id: artistId ?? null,
			artist_name: item.artist_name ?? null,
		})
		.then((result) => {
			record.historyId = result?.id ?? null;
			if (record.finished) sendFinish(record);
		})
		.catch(() => {});
}

/** Start (or switch to) a video: set it current, fetch its HLS stream, and let
 *  the persistent dock render it. Returns false if the request was superseded
 *  or the stream failed. */
export async function playVideo(
	item: VideoSessionItem,
	ctx: VideoPlayContext,
	opts: { preloaded?: PreloadedVideoStream | null; step?: boolean } = {}
): Promise<boolean> {
	// Re-selecting the video that is already playing (returning to /videos with
	// its ?videoId= still in the URL, a stale jump request, clicking its own
	// queue row) must not refetch the stream and restart it from 0:00. Refresh
	// the browse context and show it, but leave playback untouched.
	const state = get(session);
	if (ctx.resetRadio) {
		radioGeneration += 1;
		radioSeenIds = [item.tidal_id];
		radioSeenSongs = [item];
		radioRefill = null;
		const station = ctx.radioScope === 'station' ? ctx.stationId ?? null : null;
		radioStationId = station;
		radioStationTitle = station ? ctx.sourceLabel : null;
		radioStationNonce = station ? ctx.stationNonce ?? Math.random().toString(36).slice(2) : '';
		radioSeedVideoId = !ctx.radioScope || ctx.radioScope === 'artist' ? item.tidal_id : null;
	}
	const artistRadio = !ctx.radioScope || ctx.radioScope === 'artist';
	const queue = ctx.resetRadio && ctx.continuous && artistRadio ? radioStartQueue(item) : ctx.queue;
	const seed = artistRadio ? item : null;
	const radioSeedArtistId = ctx.continuous ? (ctx.resetRadio ? seed?.artist_id ?? null : state.radioSeedArtistId) : null;
	const radioSeedArtistName = ctx.continuous ? (ctx.resetRadio ? seed?.artist_name ?? null : state.radioSeedArtistName) : null;
	if (
		state.active &&
		state.current?.tidal_id === item.tidal_id &&
		Boolean(state.streamUrl) &&
		!state.error &&
		!opts.preloaded
	) {
		if (!opts.step) revealVideoStage();
		update({
			queue,
			source: sourceFor(item, ctx),
			sourceLabel: ctx.sourceLabel,
			autoplay: ctx.autoplay ?? state.autoplay,
			continuous: ctx.continuous ?? false,
			radioSeedArtistId,
			radioSeedArtistName,
			radioIssue: null,
			...(ctx.resetRadio || !ctx.continuous ? { radioSearching: false, radioDiscoveryMessage: null, radioHits: [] } : {}),
		});
		if (ctx.resetRadio && ctx.continuous) void refillVideoRadio(true);
		return true;
	}

	if (state.current && state.current.tidal_id !== item.tidal_id) finishWatch(false);
	const seq = ++streamSeq;
	// Picking something new always means "show it": the video pages scroll
	// their stage back into view. Queue steps (autoplay, next, previous) keep
	// the listener where they are.
	if (!opts.step) revealVideoStage();
	update({
		current: item,
		queue,
		source: sourceFor(item, ctx),
		sourceLabel: ctx.sourceLabel,
		autoplay: ctx.autoplay ?? get(session).autoplay,
		continuous: ctx.continuous ?? false,
		radioSeedArtistId,
		radioSeedArtistName,
		radioIssue: null,
		...(ctx.resetRadio || !ctx.continuous ? { radioSearching: false, radioDiscoveryMessage: null, radioHits: [] } : {}),
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
		if (ctx.continuous) {
			radioSeenIds.push(item.tidal_id);
			radioSeenSongs.push(item);
			radioSeenSongs = radioSeenSongs.slice(-128);
		}
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
		lastRefillBuilding = false;
		return Promise.resolve(0);
	}
	const generation = radioGeneration;
	update({ radioSearching: true, radioDiscoveryMessage: null });
	const excluded = state.queue.map((v) => v.tidal_id);
	const recentArtists = state.queue.slice(Math.max(0, state.currentIndex - 8), state.currentIndex + 1)
		.map((v) => v.artist_id).filter((id): id is number => id != null);
	const request: Promise<{ items: VideoSessionItem[]; building: boolean; exhausted: boolean }> = radioStationId
		? api.getVideoStationNext(radioStationId, {
			exclude_video_ids: excluded,
			recent_video_ids: radioSeenIds.slice(-128),
			session_nonce: radioStationNonce,
		}).then(({ items, exhausted }) => ({ items, building: false, exhausted }))
		: api.getVideoRadioNext({
			seed_artist_id: state.radioSeedArtistId,
			seed_artist_name: state.radioSeedArtistName,
			seed_video_id: radioSeedVideoId,
			exclude_video_ids: excluded,
			recent_video_ids: radioSeenIds.slice(-96),
			recent_songs: [...state.queue, ...radioSeenSongs].slice(-128).map((video) => ({
				artist_id: video.artist_id ?? null, artist_name: video.artist_name ?? null, title: video.title,
			})),
			recent_artist_ids: recentArtists,
		}).then(({ items, building }) => ({ items, building: Boolean(building), exhausted: false }));
	const pending = request.then(({ items, building, exhausted }) => {
		lastRefillBuilding = Boolean(building);
		const current = get(session);
		if (generation !== radioGeneration || !current.continuous || !current.active) return 0;
		const existing = new Set(current.queue.map((v) => v.tidal_id));
		const songs = new Set([...current.queue, ...radioSeenSongs].map(videoSongKey));
		const fresh = items.filter((item) => {
			const key = videoSongKey(item);
			if (existing.has(item.tidal_id) || songs.has(key)) return false;
			songs.add(key);
			return true;
		});
		if (fresh.length === 0) {
			update({
				radioDiscoveryMessage: exhausted && radioStationTitle
					? `You've seen everything in ${radioStationTitle}.`
					: building
						? 'Finding more videos for this station...'
						: 'No new videos in this pass. Checking again as the queue plays.',
			});
			return 0;
		}
		// Refills extend the selected queue. Keep every upcoming pick in order,
		// including the mixed opening queue supplied by library radio.
		const start = Math.max(0, current.currentIndex - 6);
		update({ queue: [...current.queue.slice(start), ...fresh] });
		const byArtist = new Map<string, number>();
		for (const item of fresh) {
			const artist = item.artist_name?.trim() || 'Unknown artist';
			byArtist.set(artist, (byArtist.get(artist) ?? 0) + 1);
		}
		const hits = [...byArtist].map(([artist, count]) => ({ artist, count }));
		update({
			radioHits: [...current.radioHits, ...hits].slice(-4),
			radioDiscoveryMessage: `${fresh.length} new ${fresh.length === 1 ? 'video' : 'videos'} added to your queue.`,
		});
		radioSeenIds.push(...fresh.map((item) => item.tidal_id));
		radioSeenIds = radioSeenIds.slice(-256);
		radioSeenSongs.push(...fresh);
		radioSeenSongs = radioSeenSongs.slice(-128);
		return fresh.length;
	}).catch(() => {
		lastRefillBuilding = false;
		if (generation === radioGeneration && get(session).continuous) {
			update({ radioDiscoveryMessage: 'Could not check for more videos. Radio will retry near the end of the queue.' });
		}
		return 0;
	});
	radioRefill = pending;
	void pending.finally(() => {
		if (radioRefill === pending) radioRefill = null;
		if (generation === radioGeneration && get(session).continuous) update({ radioSearching: false });
	});
	return pending;
}

/** Start a station: its first batch becomes the queue, refills follow. */
export async function playVideoStation(station: { id: string; title: string }): Promise<boolean> {
	const stationNonce = Math.random().toString(36).slice(2);
	const { items } = await api.getVideoStationNext(station.id, {
		exclude_video_ids: [], recent_video_ids: [], session_nonce: stationNonce,
	});
	const first = items[0];
	if (!first) return false;
	return playVideo(first, {
		queue: items, source: 'mix', sourceLabel: `${station.title} station`,
		autoplay: true, continuous: true, resetRadio: true,
		radioScope: 'station', stationId: station.id, stationNonce,
	});
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

/** Refill until a next video exists, waiting while the server is still
 *  building this station. */
async function refillUntilNext(currentId: number | undefined): Promise<VideoSessionItem | null | 'superseded'> {
	for (let attempt = 0; ; attempt += 1) {
		await refillVideoRadio(true);
		const refreshed = get(session);
		if (!refreshed.continuous || refreshed.current?.tidal_id !== currentId) return 'superseded';
		const next = refreshed.queue[findCurrentIndex(refreshed.queue, refreshed.current) + 1];
		if (next) return next;
		if (!lastRefillBuilding || attempt >= radioRetryPolicy.attempts) return null;
		await new Promise((resolve) => setTimeout(resolve, radioRetryPolicy.delayMs));
	}
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
			const replenished = await refillUntilNext(state.current?.tidal_id);
			// A click may have replaced radio while its request was in flight.
			if (replenished === 'superseded') return true;
			const refreshed = get(session);
			if (!refreshed.autoplay) return true;
			// No preloaded stream here: it belonged to a video that did not exist yet.
			if (replenished) return playVideo(replenished, {
				queue: refreshed.queue, source: refreshed.source,
				sourceLabel: refreshed.sourceLabel, autoplay: true, continuous: true,
			}, { step: true });
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
	}, { ...opts, step: true });
}

/** Return to a video already played in this session. Keeps the queue context. */
export async function previousVideo(): Promise<boolean> {
	const state = get(session);
	const previous = state.queue[state.currentIndex - 1];
	if (!previous) return false;
	return playVideo(previous, {
		queue: state.queue, source: state.source, sourceLabel: state.sourceLabel,
		autoplay: state.autoplay, continuous: state.continuous,
	}, { step: true });
}

/** Skip to the next queued video regardless of the autoplay preference. */
export async function nextVideo(): Promise<boolean> {
	const state = get(session);
	if (!state.active) return false;
	if (state.continuous && !state.queue[state.currentIndex + 1]) {
		if ((await refillUntilNext(state.current?.tidal_id)) === 'superseded') return false;
	}
	const refreshed = get(session);
	if (refreshed.current?.tidal_id !== state.current?.tidal_id) return false;
	const next = refreshed.queue[refreshed.currentIndex + 1];
	if (!next) return false;
	return playVideo(next, {
		queue: refreshed.queue, source: refreshed.source, sourceLabel: refreshed.sourceLabel,
		autoplay: refreshed.autoplay, continuous: refreshed.continuous,
	}, { step: true });
}

/** Stop the video session entirely and free the dock. */
export function clearVideoSession() {
	finishWatch(false);
	watch = null;
	streamSeq += 1;
	radioGeneration += 1;
	radioSeenIds = [];
	radioSeenSongs = [];
	radioRefill = null;
	radioSeedVideoId = null;
	clearStation();
	session.set({ ...initialState, autoplay: loadAutoplayPreference() });
}

// ─── Stage plumbing shared by the video pages and the dock ───

/** The video section's stage placeholder, published by routes/videos/+layout
 *  only while the stage is on screen. The persistent dock copies this
 *  element's rect each frame so the live player appears docked into the stage
 *  while actually being a fixed element that never unmounts on navigation.
 *  Null (stage scrolled away, or off the video pages) means the corner player. */
export const videoStageAnchor = writable<HTMLElement | null>(null);

/** The video queue panel's artwork slot. While it is on screen and large
 *  enough, the dock plays the video there instead of floating a window over
 *  the queue. Published by the layout; null when the panel is closed. */
export const videoPanelAnchor = writable<HTMLElement | null>(null);

/** Bumped whenever the listener picks a video (not on queue steps). The
 *  video section layout scrolls its stage back into view on each bump, so a
 *  click far down a tab always lands on the player. */
export const videoStageReveal = writable(0);

export function revealVideoStage() {
	videoStageReveal.update((n) => n + 1);
}
