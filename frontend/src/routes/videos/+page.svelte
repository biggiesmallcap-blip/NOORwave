<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { api, ApiError, type TidalSearchVideo, type TidalVideoMixItem } from '$lib/api/client';
	import VideoPlayer from '$lib/components/video/VideoPlayer.svelte';
	import VideoCard from '$lib/components/video/VideoCard.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { assertOnline } from '$lib/stores/player';
	import { formatTrackDuration } from '$lib/utils/format';
	import { showToast } from '$lib/stores/toast';
	import { audioSettings } from '$lib/stores/audio_settings';
	import {
		videoClearRequest,
		videoAutoplayToggleRequest,
		videoJumpRequest,
		videoSession,
		type VideoSessionSource,
	} from '$lib/stores/video_session';

	const PAGE_SIZE = 40;
	const RECENT_KEY = 'noor_recent_video_searches';
	const AUTOPLAY_KEY = 'noor_video_autoplay_next';
	const SESSION_SNAPSHOT_KEY = 'noor_video_session_snapshot';
	const RECENT_MAX = 8;
	const HINTS = ['music video', 'live session', 'official video', 'visualizer'];

	interface VideoPageSnapshot {
		selectedVideo: TidalSearchVideo | TidalVideoMixItem | null;
		videos: TidalSearchVideo[];
		mixItems: TidalVideoMixItem[];
		query: string;
		lastQuery: string;
		activeMixId: string | null;
		hasMore: boolean;
		offset: number;
		streamUrl: string | null;
		streamExpiresAt: string | null;
	}

	function saveSessionSnapshot() {
		if (typeof sessionStorage === 'undefined' || !selectedVideo) return;
		try {
			const snap: VideoPageSnapshot = {
				selectedVideo, videos, mixItems, query, lastQuery,
				activeMixId, hasMore, offset, streamUrl, streamExpiresAt,
			};
			sessionStorage.setItem(SESSION_SNAPSHOT_KEY, JSON.stringify(snap));
		} catch {}
	}

	function loadSessionSnapshot(): VideoPageSnapshot | null {
		if (typeof sessionStorage === 'undefined') return null;
		try {
			const raw = sessionStorage.getItem(SESSION_SNAPSHOT_KEY);
			return raw ? (JSON.parse(raw) as VideoPageSnapshot) : null;
		} catch { return null; }
	}

	function clearSessionSnapshot() {
		if (typeof sessionStorage === 'undefined') return;
		sessionStorage.removeItem(SESSION_SNAPSHOT_KEY);
	}

	type PrefetchedVideoStream = {
		videoId: number;
		hlsUrl: string;
		expiresAt: string | null;
	};

	let query = $state('');
	let inputEl = $state<HTMLInputElement | null>(null);
	let videos = $state<TidalSearchVideo[]>([]);
	let mixItems = $state<TidalVideoMixItem[]>([]);
	let selectedVideo = $state<TidalSearchVideo | TidalVideoMixItem | null>(null);
	let streamUrl = $state<string | null>(null);
	let streamExpiresAt = $state<string | null>(null);
	let loadingSearch = $state(false);
	let loadingMore = $state(false);
	let loadingStream = $state(false);
	let loadingMix = $state(false);
	let error = $state<string | null>(null);
	let mixError = $state<string | null>(null);
	let offset = $state(0);
	let hasMore = $state(false);
	let lastQuery = $state('');
	let activeMixId = $state<string | null>(null);
	let sentinel = $state<HTMLDivElement | null>(null);
	let recent = $state<string[]>(loadRecent());
	let autoplayNext = $state(loadAutoplayPreference());
	let debounceTimer: ReturnType<typeof setTimeout> | null = null;
	let searchAbort: AbortController | null = null;
	let streamRequestSeq = 0;
	let prefetchRequestSeq = 0;
	let prefetchedStream = $state<PrefetchedVideoStream | null>(null);
	let handledJumpNonce = 0;
	let handledAutoplayToggleNonce = 0;
	let handledClearNonce = 0;

	let heroTitle = $derived(selectedVideo?.title ?? 'TIDAL video');
	let heroArtist = $derived(selectedVideo?.artist_name ?? null);
	let videoQualityMode = $derived($audioSettings.settings?.video_quality_mode ?? 'MAX');
	let nextAutoplayVideo = $derived.by(() => getNextAutoplayVideo());
	let hasAutoplayNext = $derived.by(() => {
		const queue = activeVideoQueue();
		const index = selectedQueueIndex(queue);
		return index >= 0 && (nextAutoplayVideo !== null || (queue === videos && hasMore));
	});

	function loadRecent(): string[] {
		if (typeof localStorage === 'undefined') return [];
		try {
			const parsed = JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]');
			return Array.isArray(parsed) ? parsed.filter((item) => typeof item === 'string').slice(0, RECENT_MAX) : [];
		} catch {
			return [];
		}
	}

	function loadAutoplayPreference(): boolean {
		if (typeof localStorage === 'undefined') return false;
		return localStorage.getItem(AUTOPLAY_KEY) === 'true';
	}

	function persistAutoplayPreference() {
		if (typeof localStorage === 'undefined') return;
		localStorage.setItem(AUTOPLAY_KEY, String(autoplayNext));
	}

	function pushRecent(value: string) {
		const trimmed = value.trim();
		if (!trimmed || typeof localStorage === 'undefined') return;
		recent = [trimmed, ...recent.filter((item) => item.toLowerCase() !== trimmed.toLowerCase())].slice(0, RECENT_MAX);
		localStorage.setItem(RECENT_KEY, JSON.stringify(recent));
	}

	function clearRecent() {
		recent = [];
		if (typeof localStorage !== 'undefined') localStorage.removeItem(RECENT_KEY);
	}

	function normalizeError(errorValue: unknown, fallback: string): string {
		if (errorValue instanceof ApiError) return errorValue.message;
		if (errorValue instanceof Error) return errorValue.message;
		return fallback;
	}

	async function runSearch(nextQuery: string, replaceState = true) {
		const q = nextQuery.trim();
		searchAbort?.abort();
		searchAbort = null;
		if (!q) {
			videos = [];
			offset = 0;
			hasMore = false;
			lastQuery = '';
			error = null;
			if (replaceState) void goto('/videos', { replaceState: true, keepFocus: true });
			return;
		}

		const controller = new AbortController();
		searchAbort = controller;
		loadingSearch = true;
		error = null;
		try {
			const result = await api.searchTidalVideos(q, PAGE_SIZE, 0, controller.signal);
			if (controller.signal.aborted) return;
			videos = result.videos;
			offset = result.videos.length;
			hasMore = result.videos.length >= PAGE_SIZE;
			lastQuery = q;
			pushRecent(q);
			if (replaceState) {
				void goto(`/videos?q=${encodeURIComponent(q)}`, { replaceState: true, keepFocus: true });
			}
		} catch (err) {
			if (controller.signal.aborted || (err as Error)?.name === 'AbortError') return;
			error = normalizeError(err, 'Video search failed.');
			showToast(error, 'error', 3200);
		} finally {
			if (searchAbort === controller) searchAbort = null;
			if (!controller.signal.aborted) loadingSearch = false;
		}
	}

	function onInput() {
		if (debounceTimer) clearTimeout(debounceTimer);
		debounceTimer = setTimeout(() => void runSearch(query), 250);
	}

	async function loadMore(): Promise<number> {
		if (loadingMore || loadingSearch || !hasMore || !lastQuery) return 0;
		loadingMore = true;
		try {
			const result = await api.searchTidalVideos(lastQuery, PAGE_SIZE, offset);
			const seen = new Set(videos.map((video) => video.tidal_id));
			const fresh = result.videos.filter((video) => !seen.has(video.tidal_id));
			videos = [...videos, ...fresh];
			offset += result.videos.length;
			hasMore = result.videos.length >= PAGE_SIZE;
			return fresh.length;
		} catch (err) {
			hasMore = false;
			showToast(normalizeError(err, 'Could not load more videos.'), 'error', 2800);
			return 0;
		} finally {
			loadingMore = false;
		}
	}

	async function fetchStream(videoId: number): Promise<string> {
		if (!assertOnline()) throw new Error('Server is reconnecting.');
		const seq = ++streamRequestSeq;
		const stream = await api.getTidalVideoStream(videoId);
		if (seq !== streamRequestSeq) throw new Error('Route changed before video loaded.');
		streamUrl = stream.hls_url;
		streamExpiresAt = stream.expires_at;
		return stream.hls_url;
	}

	async function selectVideo(
		video: TidalSearchVideo | TidalVideoMixItem,
		updateUrl = true,
		options: { keepCurrentStream?: boolean; preloaded?: PrefetchedVideoStream | null } = {}
	): Promise<boolean> {
		const previousVideo = selectedVideo;
		const previousStreamUrl = streamUrl;
		const previousExpiresAt = streamExpiresAt;
		selectedVideo = video;
		if (!options.keepCurrentStream) streamUrl = null;
		streamExpiresAt = null;
		error = null;
		loadingStream = true;
		try {
			if (options.preloaded?.videoId === video.tidal_id) {
				streamRequestSeq += 1;
				streamUrl = options.preloaded.hlsUrl;
				streamExpiresAt = options.preloaded.expiresAt;
				prefetchedStream = null;
			} else {
				await fetchStream(video.tidal_id);
			}
			if (updateUrl) {
				const params = new URLSearchParams();
				if ('mix_id' in video && video.mix_id) {
					params.set('mixId', String(video.mix_id));
				} else if (lastQuery) {
					params.set('q', lastQuery);
				}
				params.set('videoId', String(video.tidal_id));
				void goto(`/videos?${params.toString()}`, { keepFocus: true });
			}
			return true;
		} catch (err) {
			if (options.keepCurrentStream) {
				selectedVideo = previousVideo;
				streamUrl = previousStreamUrl;
				streamExpiresAt = previousExpiresAt;
			}
			error = normalizeError(err, 'This video could not be loaded.');
			showToast(error, 'error', 3200);
			return false;
		} finally {
			loadingStream = false;
		}
	}

	async function refreshSelectedStream() {
		if (!selectedVideo) throw new Error('No video selected.');
		return fetchStream(selectedVideo.tidal_id);
	}

	async function loadMix(mixId: string, autoPlayFirst = false) {
		loadingMix = true;
		mixError = null;
		activeMixId = mixId;
		mixItems = [];
		try {
			const result = await api.getTidalVideoMixItems(mixId);
			mixItems = result.items.map((item) => ({ ...item, mix_id: mixId }));
			if (mixItems.length === 0) mixError = 'This mix did not return video items.';
			if (autoPlayFirst && mixItems.length > 0) {
				autoplayNext = true;
				persistAutoplayPreference();
				await selectVideo(mixItems[0], false);
			}
		} catch (err) {
			mixError = normalizeError(err, 'Video mix items could not load.');
			showToast(mixError, 'error', 3200);
		} finally {
			loadingMix = false;
		}
	}

	function pickSearch(value: string) {
		query = value;
		void runSearch(value);
		inputEl?.focus();
	}

	function selectCard(video: TidalSearchVideo | TidalVideoMixItem) {
		void selectVideo(video);
	}

	function activeVideoQueue(): (TidalSearchVideo | TidalVideoMixItem)[] {
		if (selectedVideo && 'mix_id' in selectedVideo && selectedVideo.mix_id != null) return mixItems;
		return videos;
	}

	function selectedQueueIndex(queue: (TidalSearchVideo | TidalVideoMixItem)[]): number {
		if (!selectedVideo) return -1;
		return queue.findIndex((item) => item.tidal_id === selectedVideo?.tidal_id);
	}

	function getNextAutoplayVideo(): TidalSearchVideo | TidalVideoMixItem | null {
		const queue = activeVideoQueue();
		const index = selectedQueueIndex(queue);
		if (index < 0) return null;
		return queue[index + 1] ?? null;
	}

	function videoSessionSource(): VideoSessionSource {
		if (selectedVideo && 'mix_id' in selectedVideo && selectedVideo.mix_id != null) return 'mix';
		if (lastQuery) return 'search';
		if (selectedVideo) return 'direct';
		return activeMixId ? 'mix' : 'none';
	}

	function videoSessionSourceLabel(): string | null {
		const source = videoSessionSource();
		if (source === 'mix') return activeMixId ? `Video mix ${activeMixId}` : 'Video mix';
		if (source === 'search') return lastQuery;
		if (source === 'direct') return 'Direct video';
		return null;
	}

	function findVideoInCurrentContext(videoId: number): TidalSearchVideo | TidalVideoMixItem | null {
		return [...mixItems, ...videos].find((item) => item.tidal_id === videoId) ?? null;
	}

	function toggleVideoAutoplay() {
		if (!autoplayNext && !hasAutoplayNext) {
			showToast('Choose a video from search results or a video mix first.', 'info', 2400);
			return;
		}
		autoplayNext = !autoplayNext;
		persistAutoplayPreference();
	}

	async function prefetchNextStream(video: TidalSearchVideo | TidalVideoMixItem) {
		if (prefetchedStream?.videoId === video.tidal_id) return;
		if (!assertOnline()) return;
		const seq = ++prefetchRequestSeq;
		try {
			const stream = await api.getTidalVideoStream(video.tidal_id);
			if (seq !== prefetchRequestSeq || nextAutoplayVideo?.tidal_id !== video.tidal_id) return;
			prefetchedStream = {
				videoId: video.tidal_id,
				hlsUrl: stream.hls_url,
				expiresAt: stream.expires_at,
			};
		} catch {
			if (seq === prefetchRequestSeq) prefetchedStream = null;
		}
	}

	async function handleVideoEnded() {
		if (!autoplayNext) return;
		let queue = activeVideoQueue();
		let index = selectedQueueIndex(queue);
		if (index < 0) {
			autoplayNext = false;
			persistAutoplayPreference();
			return;
		}

		if (index >= queue.length - 1 && queue === videos && hasMore) {
			await loadMore();
			queue = activeVideoQueue();
			index = selectedQueueIndex(queue);
		}

		const next = queue[index + 1];
		if (!next) {
			showToast('End of video results.', 'info', 2200);
			autoplayNext = false;
			persistAutoplayPreference();
			return;
		}

		const preloaded = prefetchedStream?.videoId === next.tidal_id ? prefetchedStream : null;
		const loaded = await selectVideo(next, true, { keepCurrentStream: true, preloaded });
		if (!loaded) {
			autoplayNext = false;
			persistAutoplayPreference();
		}
	}

	async function parseUrl() {
		const params = new URLSearchParams(window.location.search);
		const q = params.get('q') ?? '';
		const videoId = Number(params.get('videoId'));
		const mixId = params.get('mixId');
		const shouldPlayMix = params.get('play') === '1';
		query = q;
		if (q) void runSearch(q, false);
		if (mixId) {
			await loadMix(mixId, shouldPlayMix);
			if (!shouldPlayMix && Number.isFinite(videoId) && videoId > 0) {
				const fromMix = mixItems.find((item) => item.tidal_id === videoId);
				if (fromMix) {
					void selectVideo(fromMix, false);
					return;
				}
			}
		}
		if (Number.isFinite(videoId) && videoId > 0) {
			void selectVideo({
				tidal_id: videoId,
				title: `TIDAL video ${videoId}`,
				duration_ms: null,
				artist_id: null,
				artist_name: null,
				album_tidal_id: null,
				artwork_url: null,
				quality: null,
				explicit: null,
				type: 'video',
			}, false);
		}
	}

	onMount(() => {
		void audioSettings.load();
		const params = new URLSearchParams(window.location.search);
		const hasExplicitParams = params.has('q') || params.has('videoId') || params.has('mixId');

		if (!hasExplicitParams) {
			const snap = loadSessionSnapshot();
			if (snap?.selectedVideo) {
				selectedVideo = snap.selectedVideo;
				videos = snap.videos ?? [];
				mixItems = snap.mixItems ?? [];
				query = snap.query ?? '';
				lastQuery = snap.lastQuery ?? '';
				activeMixId = snap.activeMixId ?? null;
				hasMore = snap.hasMore ?? false;
				offset = snap.offset ?? 0;

				const streamFresh =
					snap.streamUrl &&
					snap.streamExpiresAt &&
					new Date(snap.streamExpiresAt).getTime() > Date.now() + 30_000;
				if (streamFresh) {
					streamUrl = snap.streamUrl;
					streamExpiresAt = snap.streamExpiresAt;
				} else {
					void selectVideo(snap.selectedVideo, false);
				}

				const onPop = () => void parseUrl();
				window.addEventListener('popstate', onPop);
				return () => window.removeEventListener('popstate', onPop);
			}
		}

		void parseUrl();
		const onPop = () => void parseUrl();
		window.addEventListener('popstate', onPop);
		return () => window.removeEventListener('popstate', onPop);
	});

	$effect(() => {
		if (!sentinel) return;
		const observer = new IntersectionObserver((entries) => {
			if (entries.some((entry) => entry.isIntersecting)) void loadMore();
		}, { rootMargin: '480px 0px' });
		observer.observe(sentinel);
		return () => observer.disconnect();
	});

	$effect(() => {
		if (!autoplayNext || !nextAutoplayVideo) {
			prefetchRequestSeq += 1;
			prefetchedStream = null;
			return;
		}
		void prefetchNextStream(nextAutoplayVideo);
	});

	$effect(() => {
		videoSession.sync({
			current: selectedVideo,
			queue: activeVideoQueue(),
			source: videoSessionSource(),
			sourceLabel: videoSessionSourceLabel(),
			autoplay: autoplayNext,
			loading: loadingSearch || loadingMore || loadingStream || loadingMix,
			error: error ?? mixError,
		});
		if (selectedVideo) saveSessionSnapshot();
		else clearSessionSnapshot();
	});

	$effect(() => {
		const request = $videoJumpRequest;
		if (!request || request.nonce === handledJumpNonce) return;
		handledJumpNonce = request.nonce;
		const next = findVideoInCurrentContext(request.videoId);
		if (!next) return;
		void selectVideo(next);
	});

	$effect(() => {
		const nonce = $videoAutoplayToggleRequest;
		if (nonce === handledAutoplayToggleNonce) return;
		handledAutoplayToggleNonce = nonce;
		toggleVideoAutoplay();
	});

	$effect(() => {
		const nonce = $videoClearRequest;
		if (nonce === handledClearNonce) return;
		handledClearNonce = nonce;
		videos = [];
		mixItems = [];
		query = '';
		lastQuery = '';
		activeMixId = null;
		hasMore = false;
		offset = 0;
		error = null;
		mixError = null;
	});

	onDestroy(() => {
		if (debounceTimer) clearTimeout(debounceTimer);
		searchAbort?.abort();
		streamRequestSeq += 1;
		prefetchRequestSeq += 1;
	});
</script>

<div class="videos-page">
	<header class="search-header">
		<input
			bind:this={inputEl}
			class="search-input"
			type="text"
			placeholder="Search TIDAL videos"
			bind:value={query}
			oninput={onInput}
		/>
	</header>

	<section class="hero glass-panel">
		<div class="player-shell">
			{#if loadingStream && !streamUrl}
				<Skeleton rows={4} label="Loading video" />
			{:else if streamUrl}
				<VideoPlayer
					src={streamUrl}
					poster={selectedVideo?.artwork_url}
					title={heroTitle}
					artist={heroArtist}
					qualityMode={videoQualityMode}
					autoplayNext={autoplayNext}
					hasNext={hasAutoplayNext}
					upNextTitle={nextAutoplayVideo?.title ?? null}
					upNextArtist={nextAutoplayVideo?.artist_name ?? null}
					onToggleAutoplay={toggleVideoAutoplay}
					onEnded={handleVideoEnded}
					refreshStream={refreshSelectedStream}
				/>
			{:else}
				<EmptyState title="Choose a video" copy="Search TIDAL videos, then select one to play here." />
			{/if}
		</div>
		<div class="hero-meta">
			<p class="eyebrow">Videos</p>
			<h1>{heroTitle}</h1>
			{#if selectedVideo}
				<div class="meta-line">
					{#if selectedVideo.artist_name}
						<button
							type="button"
							class="meta-link"
							oncontextmenu={(event) => {
								if (selectedVideo?.artist_id == null) return;
								openContextMenu(
									event,
									buildArtistMenu({ tidal_id: selectedVideo.artist_id, name: selectedVideo.artist_name ?? 'Artist' }, { isLocal: false }),
									selectedVideo.artist_name ?? undefined
								);
							}}
							onclick={() => {
								if (selectedVideo?.artist_id != null) void goto(`/tidal/artists/${selectedVideo.artist_id}`);
							}}
						>
							{selectedVideo.artist_name}
						</button>
					{/if}
					{#if selectedVideo.duration_ms}
						<span>{formatTrackDuration(selectedVideo.duration_ms)}</span>
					{/if}
					{#if streamExpiresAt}
						<span>Stream ready</span>
					{/if}
				</div>
			{:else}
				<p class="page-copy">A focused TIDAL video surface with audio queue state preserved.</p>
			{/if}
			{#if error}
				<p class="inline-error">{error}</p>
			{/if}
		</div>
	</section>

	{#if !query.trim() && videos.length === 0 && mixItems.length === 0}
		<section class="landing-row">
			{#if recent.length > 0}
				<div class="rail-block">
					<div class="rail-header">
						<span class="eyebrow">Recent</span>
						<button type="button" class="text-btn" onclick={clearRecent}>Clear</button>
					</div>
					<div class="chips">
						{#each recent as item (item)}
							<button type="button" class="hint-chip" onclick={() => pickSearch(item)}>{item}</button>
						{/each}
					</div>
				</div>
			{/if}
			<div class="rail-block">
				<span class="eyebrow">Try</span>
				<div class="chips">
					{#each HINTS as item (item)}
						<button type="button" class="hint-chip" onclick={() => pickSearch(item)}>{item}</button>
					{/each}
				</div>
			</div>
		</section>
	{/if}

	{#if loadingSearch}
		<div class="status-wrap"><Skeleton rows={4} label="Searching videos" /></div>
	{:else if error && query.trim() && videos.length === 0}
		<EmptyState title="Video search failed" copy={error} />
	{:else if query.trim() && videos.length === 0}
		<EmptyState title="No videos found" copy="Try a broader artist, song, or live-session search." />
	{/if}

	{#if videos.length > 0}
		<section class="results-section">
			<div class="section-heading">
				<p class="eyebrow">Results</p>
				<h2>{lastQuery}</h2>
			</div>
			<div class="video-grid">
				{#each videos as video (video.tidal_id)}
					<VideoCard {video} onSelect={(item) => !('id' in item) && selectCard(item)} />
				{/each}
			</div>
			<div bind:this={sentinel} class="infinite-sentinel" aria-hidden="true">
				{#if loadingMore}<span>Loading more…</span>{/if}
			</div>
		</section>
	{/if}

	{#if loadingMix || mixItems.length > 0 || mixError}
		<section class="results-section">
			<div class="section-heading">
				<p class="eyebrow">Video mix</p>
				<h2>Mix items</h2>
			</div>
			{#if loadingMix}
				<Skeleton rows={3} label="Loading mix" />
			{:else if mixError}
				<EmptyState title="Mix unavailable" copy={mixError} />
			{:else}
				<div class="video-grid">
					{#each mixItems as video (`${video.mix_id}-${video.tidal_id}`)}
						<VideoCard {video} onSelect={(item) => !('id' in item) && selectCard(item)} />
					{/each}
				</div>
			{/if}
		</section>
	{/if}
</div>

<style>
	.videos-page {
		width: min(100%, var(--content-width));
		margin: 0 auto;
		display: grid;
		gap: 28px;
		padding-bottom: max(44px, var(--safe-bottom));
	}

	.search-header {
		padding: 0 4px;
	}

	.search-input {
		display: block;
		width: 100%;
		max-width: 640px;
		margin: 0 auto;
		background: var(--bg-raised);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-lg);
		padding: 12px 22px;
		font-size: 15px;
		color: var(--text-primary);
		outline: none;
		transition: border-color 0.15s, background 0.15s;
	}

	.search-input::placeholder {
		color: var(--text-tertiary);
	}

	.search-input:focus {
		border-color: var(--accent-line);
		background: var(--input-focus);
	}

	.hero {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(260px, 340px);
		gap: 20px;
		padding: 16px;
	}

	.player-shell {
		min-width: 0;
	}

	.hero-meta {
		display: flex;
		flex-direction: column;
		justify-content: center;
		gap: 10px;
		min-width: 0;
	}

	.hero-meta h1 {
		margin: 0;
		font-size: clamp(1.35rem, 2vw, 1.9rem);
	}

	.meta-line {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		align-items: center;
		color: var(--text-tertiary);
		font-size: 0.82rem;
	}

	.meta-link,
	.text-btn {
		color: var(--accent-strong);
		font-weight: 700;
	}

	.inline-error {
		color: var(--state-error);
		font-size: 0.82rem;
	}

	.landing-row,
	.results-section {
		display: grid;
		gap: 14px;
	}

	.rail-block {
		display: grid;
		gap: 10px;
	}

	.rail-header,
	.section-heading {
		display: flex;
		align-items: end;
		justify-content: space-between;
		gap: 12px;
	}

	.section-heading {
		align-items: baseline;
		justify-content: flex-start;
	}

	.section-heading h2 {
		margin: 0;
		color: var(--text-secondary);
		font-size: 1rem;
	}

	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.hint-chip {
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		border-radius: 999px;
		padding: 7px 13px;
		font-size: 0.76rem;
		font-weight: 700;
	}

	.hint-chip:hover {
		background: var(--accent-soft);
		color: var(--accent-strong);
		border-color: var(--accent-line);
	}

	.video-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
		gap: 14px;
	}

	.status-wrap {
		padding: 12px;
	}

	.infinite-sentinel {
		min-height: 24px;
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
		font-size: 0.76rem;
	}

	@media (max-width: 860px) {
		.hero {
			grid-template-columns: 1fr;
		}
	}

	@media (max-width: 620px) {
		.videos-page {
			gap: 20px;
		}

		.hero {
			padding: 10px;
		}

		.video-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
