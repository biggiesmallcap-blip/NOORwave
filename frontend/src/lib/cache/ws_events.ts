import { cacheKeys, invalidateHomeCaches, invalidateLibraryCaches, patchDiscoveryProgress } from '$lib/cache/api_queries';
import { dataCache, stableCacheKey, type CacheKeyInput } from '$lib/cache/query';

type CacheWsMessage =
	| { type: string; [key: string]: unknown }
	| null
	| undefined;

const refetchTimers = new Map<string, ReturnType<typeof setTimeout>>();

function debounceRefetch(key: CacheKeyInput, delayMs = 150): void {
	const normalized = stableCacheKey(key);
	const existing = refetchTimers.get(normalized);
	if (existing) clearTimeout(existing);
	refetchTimers.set(
		normalized,
		setTimeout(() => {
			refetchTimers.delete(normalized);
			dataCache.invalidateKey(normalized, { refetch: true });
		}, delayMs),
	);
}

function invalidateGenreCaches(): void {
	dataCache.invalidatePrefix(['api', 'getGenreGalaxySnapshot']);
	dataCache.invalidatePrefix(['api', 'getGenreHeat']);
	dataCache.invalidatePrefix(['api', 'getGenreCohorts']);
	dataCache.invalidatePrefix(['api', 'getGenreEvolution']);
	dataCache.invalidatePrefix(['api', 'getGenreAudioMetrics']);
	dataCache.invalidatePrefix(['api', 'getGenreTracks']);
}

function invalidatePlaylistCaches(): void {
	dataCache.invalidatePrefix(['api', 'getPlaylists']);
	dataCache.invalidatePrefix(['api', 'getPlaylistTracks']);
	dataCache.invalidatePrefix(['api', 'evaluateSmartPlaylist']);
	dataCache.invalidatePrefix(['api', 'getPlaylistCoverSample']);
}

export function applyCacheUpdateForWsMessage(message: CacheWsMessage): void {
	if (!message || typeof message.type !== 'string') return;

	if (message.type === 'queue_updated') {
		debounceRefetch(cacheKeys.playbackState(), 100);
		return;
	}

	if (
		message.type === 'playback_changed' ||
		message.type === 'track_changed' ||
		message.type === 'playback_failed'
	) {
		debounceRefetch(cacheKeys.playbackState(), 100);
		debounceRefetch(cacheKeys.playbackRuntime(), 150);
		return;
	}

	if (message.type === 'listen_history_updated') {
		dataCache.invalidatePrefix(['api', 'getRecentListens']);
		dataCache.invalidatePrefix(['api', 'getTracks']);
		dataCache.invalidatePrefix(['api', 'search']);
		debounceRefetch(cacheKeys.genreHeat(90), 250);
		return;
	}

	if (message.type === 'library_synced') {
		invalidateLibraryCaches();
		invalidatePlaylistCaches();
		invalidateGenreCaches();
		invalidateHomeCaches();
		return;
	}

	if (message.type === 'musicbrainz_enriched') {
		debounceRefetch(cacheKeys.settings.musicBrainzStatus(), 250);
		invalidateGenreCaches();
		return;
	}

	if (message.type === 'training_progress') {
		patchDiscoveryProgress({
			stage: typeof message.stage === 'string' ? message.stage : undefined,
			progress: typeof message.progress === 'number' ? message.progress : undefined,
			tracks_done: typeof message.tracks_done === 'number' ? message.tracks_done : undefined,
			tracks_total: typeof message.tracks_total === 'number' ? message.tracks_total : undefined,
		});
		if (typeof message.progress === 'number' && message.progress >= 0.95) {
			debounceRefetch(cacheKeys.settings.discoveryStatus(), 1000);
		}
		return;
	}

	if (message.type === 'radio_similarity_computed') {
		dataCache.patch<{ row_count: number; built_at: string | null }>(
			cacheKeys.settings.radioSimilarityStatus(),
			(current) => {
				if (!current) return current;
				return {
					...current,
					row_count: typeof message.pairs === 'number' ? message.pairs : current.row_count,
				};
			},
		);
		debounceRefetch(cacheKeys.settings.radioSimilarityStatus(), 500);
		return;
	}

	if (message.type === 'audio_analysis_complete' || message.type === 'track_analyzed') {
		debounceRefetch(cacheKeys.settings.audioFeaturesStats(), 250);
		dataCache.invalidatePrefix(['api', 'getGenreAudioMetrics']);
		return;
	}

	if (message.type === 'acrcloud_scan_progress' || message.type === 'acrcloud_scan_complete') {
		debounceRefetch(cacheKeys.settings.acrCloudStatus(), 250);
	}
}

export function clearWsCacheTimers(): void {
	for (const timer of refetchTimers.values()) clearTimeout(timer);
	refetchTimers.clear();
}
