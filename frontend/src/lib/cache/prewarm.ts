import { base } from '$app/paths';
import { preloadCode } from '$app/navigation';
import { cachedApi } from '$lib/cache/api_queries';

export interface PrewarmOptions {
	enabled?: boolean;
	delayMs?: number;
	routes?: string[];
}

const DEFAULT_ROUTES = ['/', '/library', '/search', '/playlists', '/genres', '/settings'];

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
	return scheduleIdle(() => {
		for (const route of routes) {
			void preloadCode(base + route).catch(() => undefined);
		}

		void cachedApi.getHomeArticles().catch(() => undefined);
		void cachedApi.getHomeNews().catch(() => undefined);
		void cachedApi.getTracks('date_added', 'desc', 100, 0, true, false).catch(() => undefined);
		void cachedApi.getAlbums('title', 'asc', 100, 0, true).catch(() => undefined);
		void cachedApi.getGenres().catch(() => undefined);
		void cachedApi.getGenreGalaxySnapshot(90).catch(() => undefined);
		void cachedApi.getPlaylists().catch(() => undefined);
		void cachedApi.getDiscoveryStatus().catch(() => undefined);
		void cachedApi.getPlaybackRuntime().catch(() => undefined);
	}, delayMs);
}
