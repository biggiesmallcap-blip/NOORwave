import { base } from '$app/paths';
import { preloadCode } from '$app/navigation';
import { cachedApi } from '$lib/cache/api_queries';

export interface PrewarmOptions {
	enabled?: boolean;
	delayMs?: number;
	routes?: string[];
}

const DEFAULT_ROUTES = ['/', '/library', '/search'];

function scheduleIdle(task: () => void, delayMs: number): () => void {
	if (typeof window === 'undefined') return () => {};
	let idleId: number | null = null;
	const timer = window.setTimeout(() => {
		const idle = window.requestIdleCallback;
		if (typeof idle === 'function') {
			idleId = idle(task, { timeout: 1000 });
			return;
		}
		task();
	}, delayMs);
	return () => {
		window.clearTimeout(timer);
		if (idleId !== null) window.cancelIdleCallback?.(idleId);
	};
}

export function scheduleStartupPrewarm(options: PrewarmOptions = {}): () => void {
	if (options.enabled === false) return () => {};
	const routes = options.routes ?? DEFAULT_ROUTES;
	const delayMs = options.delayMs ?? 1400;

	// Wave 1: route code + the lists the user is most likely to open next. Off the
	// critical path (idle callback) so first paint wins. The home shelves
	// (mixes/radio/recommendations) are intentionally NOT warmed here - their
	// components revalidate on mount, so prewarming them would double-fetch.
	const cancelWave1 = scheduleIdle(() => {
		for (const route of routes) {
			void preloadCode(base + route).catch(() => undefined);
		}
		void cachedApi.getTracks('date_added', 'desc', 100, 0, true, false).catch(() => undefined);
		void cachedApi.getAlbums('title', 'asc', 100, 0, true).catch(() => undefined);
		void cachedApi.getPlaylists().catch(() => undefined);
		void cachedApi.getGenres().catch(() => undefined);
		void cachedApi.getDiscoveryStatus().catch(() => undefined);
		void cachedApi.getPlaybackRuntime().catch(() => undefined);
	}, delayMs);

	// Wave 2: heavier / less-urgent warmups deferred further so they don't contend
	// with first interaction - the ~100KB genre galaxy snapshot, plus home
	// articles/news which the home page already loads reactively on mount.
	const cancelWave2 = scheduleIdle(() => {
		void cachedApi.getGenreGalaxySnapshot(90).catch(() => undefined);
		void cachedApi.getHomeArticles().catch(() => undefined);
		void cachedApi.getHomeNews().catch(() => undefined);
	}, delayMs + 2400);

	return () => {
		cancelWave1();
		cancelWave2();
	};
}
