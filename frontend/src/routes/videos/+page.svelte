<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { api, ApiError, type TidalSearchVideo, type TidalVideoMixItem } from '$lib/api/client';
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
		videoStageAnchor,
		playVideo,
		clearVideoSession,
		type VideoSessionItem,
		type VideoSessionSource,
	} from '$lib/stores/video_session';

	const PAGE_SIZE = 40;
	const RECENT_KEY = 'noor_recent_video_searches';
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

	function snapshotHasRestorableContext(snap: VideoPageSnapshot): boolean {
		return Boolean(
			snap.query?.trim() ||
			snap.lastQuery?.trim() ||
			snap.activeMixId ||
			(snap.videos?.length ?? 0) > 0 ||
			(snap.mixItems?.length ?? 0) > 0
		);
	}

	let query = $state('');
	let inputEl = $state<HTMLInputElement | null>(null);
	let videos = $state<TidalSearchVideo[]>([]);
	let mixItems = $state<TidalVideoMixItem[]>([]);
	let loadingSearch = $state(false);
	let loadingMore = $state(false);
	let loadingMix = $state(false);
	let error = $state<string | null>(null);
	let mixError = $state<string | null>(null);
	let offset = $state(0);
	let hasMore = $state(false);
	let lastQuery = $state('');
	let activeMixId = $state<string | null>(null);
	let sentinel = $state<HTMLDivElement | null>(null);
	let recent = $state<string[]>(loadRecent());
	let stageAnchor = $state<HTMLDivElement | null>(null);
	let debounceTimer: ReturnType<typeof setTimeout> | null = null;
	let searchAbort: AbortController | null = null;
	let loadMoreSeq = 0;
	let mixLoadSeq = 0;
	let handledJumpNonce = 0;
	let handledAutoplayToggleNonce = 0;
	let handledClearNonce = 0;

	// The playing video lives in the persistent dock + store; the route reads it
	// for hero meta and card highlighting, and writes picks via playVideo().
	let selectedVideo = $derived($videoSession.current);
	let streamUrl = $derived($videoSession.streamUrl);
	let streamExpiresAt = $derived($videoSession.streamExpiresAt);
	let loadingStream = $derived($videoSession.loading);
	let autoplayNext = $derived($videoSession.autoplay);

	let heroTitle = $derived(selectedVideo?.title ?? 'TIDAL video');
	let heroArtist = $derived(selectedVideo?.artist_name ?? null);
	let hasVideoChoices = $derived(videos.length > 0 || mixItems.length > 0);
	let showChooseVideoPrompt = $derived(
		!selectedVideo &&
		!streamUrl &&
		!loadingStream &&
		query.trim().length > 0 &&
		hasVideoChoices
	);
	let showVideoHero = $derived(Boolean(selectedVideo || streamUrl || loadingStream || showChooseVideoPrompt));

	function loadRecent(): string[] {
		if (typeof localStorage === 'undefined') return [];
		try {
			const parsed = JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]');
			return Array.isArray(parsed) ? parsed.filter((item) => typeof item === 'string').slice(0, RECENT_MAX) : [];
		} catch {
			return [];
		}
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

	function clearVideoPageSession() {
		videos = [];
		mixItems = [];
		query = '';
		lastQuery = '';
		activeMixId = null;
		hasMore = false;
		offset = 0;
		error = null;
		mixError = null;
		loadMoreSeq += 1;
		mixLoadSeq += 1;
		clearSessionSnapshot();
		clearVideoSession();
		void goto('/videos', { replaceState: true, keepFocus: true });
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
		loadMoreSeq += 1;
		loadingMore = false;
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
		loadMoreSeq += 1;
		loadingMore = false;
		debounceTimer = setTimeout(() => void runSearch(query), 250);
	}

	async function loadMore(): Promise<number> {
		if (loadingMore || loadingSearch || !hasMore || !lastQuery) return 0;
		const seq = ++loadMoreSeq;
		const pageQuery = lastQuery;
		const pageOffset = offset;
		const isCurrentLoadMore = () =>
			seq === loadMoreSeq &&
			lastQuery === pageQuery &&
			offset === pageOffset;
		loadingMore = true;
		try {
			const result = await api.searchTidalVideos(pageQuery, PAGE_SIZE, pageOffset);
			if (!isCurrentLoadMore()) return 0;
			const seen = new Set(videos.map((video) => video.tidal_id));
			const fresh = result.videos.filter((video) => !seen.has(video.tidal_id));
			videos = [...videos, ...fresh];
			offset += result.videos.length;
			hasMore = result.videos.length >= PAGE_SIZE;
			return fresh.length;
		} catch (err) {
			if (!isCurrentLoadMore()) return 0;
			hasMore = false;
			showToast(normalizeError(err, 'Could not load more videos.'), 'error', 2800);
			return 0;
		} finally {
			if (seq === loadMoreSeq) loadingMore = false;
		}
	}

	function buildPlayContext(video: VideoSessionItem) {
		const isMix = 'mix_id' in video && video.mix_id != null;
		return {
			queue: isMix ? mixItems : videos,
			source: (isMix ? 'mix' : lastQuery ? 'search' : 'direct') as VideoSessionSource,
			sourceLabel: isMix
				? activeMixId
					? `Video mix ${activeMixId}`
					: 'Video mix'
				: lastQuery || null,
			autoplay: $videoSession.autoplay,
		};
	}

	async function selectVideo(video: VideoSessionItem, updateUrl = true): Promise<boolean> {
		if (!assertOnline()) {
			showToast('Server is reconnecting.', 'error', 3200);
			return false;
		}
		const ok = await playVideo(video, buildPlayContext(video));
		if (!ok) {
			showToast($videoSession.error ?? 'This video could not be loaded.', 'error', 3200);
			return false;
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
	}

	async function loadMix(mixId: string, autoPlayFirst = false) {
		const seq = ++mixLoadSeq;
		const isCurrentMixLoad = () => seq === mixLoadSeq && activeMixId === mixId;
		loadMoreSeq += 1;
		loadingMore = false;
		loadingMix = true;
		mixError = null;
		activeMixId = mixId;
		mixItems = [];
		try {
			const result = await api.getTidalVideoMixItems(mixId);
			if (!isCurrentMixLoad()) return;
			mixItems = result.items.map((item) => ({ ...item, mix_id: mixId }));
			if (mixItems.length === 0) mixError = 'This mix did not return video items.';
			if (autoPlayFirst && isCurrentMixLoad() && mixItems.length > 0) {
				videoSession.setAutoplay(true);
				await selectVideo(mixItems[0], false);
			}
		} catch (err) {
			if (!isCurrentMixLoad()) return;
			mixError = normalizeError(err, 'Video mix items could not load.');
			if (isCurrentMixLoad()) showToast(mixError, 'error', 3200);
		} finally {
			if (seq === mixLoadSeq) loadingMix = false;
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

	function findVideoInCurrentContext(videoId: number): VideoSessionItem | null {
		return [...mixItems, ...videos].find((item) => item.tidal_id === videoId) ?? null;
	}

	function toggleVideoAutoplay() {
		videoSession.setAutoplay(!$videoSession.autoplay);
	}

	async function parseUrl() {
		const params = new URLSearchParams(window.location.search);
		const q = params.get('q') ?? '';
		const videoId = Number(params.get('videoId'));
		const mixId = params.get('mixId');
		const shouldPlayMix = params.get('play') === '1';
		query = q;
		if (q) await runSearch(q, false);
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
			const fromContext = findVideoInCurrentContext(videoId);
			if (fromContext) {
				void selectVideo(fromContext, false);
				return;
			}
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
			if (snap?.selectedVideo && snapshotHasRestorableContext(snap)) {
				videos = snap.videos ?? [];
				mixItems = snap.mixItems ?? [];
				query = snap.query ?? '';
				lastQuery = snap.lastQuery ?? '';
				activeMixId = snap.activeMixId ?? null;
				hasMore = snap.hasMore ?? false;
				offset = snap.offset ?? 0;

				// The dock unmounted on full reload, so rehydrate playback through
				// the store (re-fetches a fresh stream rather than trusting a
				// possibly-expired snapshot URL).
				if (!$videoSession.active) void selectVideo(snap.selectedVideo, false);

				const onPop = () => void parseUrl();
				window.addEventListener('popstate', onPop);
				return () => window.removeEventListener('popstate', onPop);
			}
			clearSessionSnapshot();
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

	// Hand the route's hero placeholder to the persistent dock so it can dock
	// the live player into it while on /videos.
	$effect(() => {
		videoStageAnchor.set(stageAnchor);
		return () => videoStageAnchor.set(null);
	});

	// Snapshot for full-page reload recovery (in-app nav is handled by the
	// persistent dock, which never unmounts).
	$effect(() => {
		if (selectedVideo && (lastQuery || activeMixId)) saveSessionSnapshot();
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
		clearVideoPageSession();
	});

	onDestroy(() => {
		if (debounceTimer) clearTimeout(debounceTimer);
		searchAbort?.abort();
		mixLoadSeq += 1;
	});
</script>

<div class="videos-page">
	<header class="search-header">
		<div class="search-tools">
			<input
				bind:this={inputEl}
				class="search-input"
				type="search"
				placeholder="Search TIDAL videos"
				bind:value={query}
				oninput={onInput}
			/>
			<a class="editorial-link" href="/tidal/videos">TIDAL editorial</a>
		</div>
	</header>

	{#if showVideoHero}
	<section class="hero glass-panel" class:hero--prompt={showChooseVideoPrompt}>
		<div class="player-shell">
			<!-- Placeholder the persistent dock positions its live player over while
			     on /videos. The actual <video> lives in VideoDock so it survives
			     navigation. -->
			<div class="stage-anchor" class:has-video={Boolean(streamUrl)} bind:this={stageAnchor}>
				{#if loadingStream && !streamUrl}
					<Skeleton rows={4} label="Loading video" />
				{:else if showChooseVideoPrompt}
					<div class="video-choice-prompt" aria-live="polite">
						<strong>Choose a video</strong>
						<span>Select any result to start playback.</span>
					</div>
				{/if}
			</div>
		</div>
		{#if !showChooseVideoPrompt}
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
								event.preventDefault();
								event.stopPropagation();
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
			{/if}
			{#if error}
				<p class="inline-error">{error}</p>
			{/if}
		</div>
		{/if}
	</section>
	{/if}

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
		width: 100%;
		max-width: var(--content-width);
		margin: 0 auto var(--space-5);
		padding: 0 4px;
	}

	.search-tools {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-3);
		flex-wrap: wrap;
	}

	.search-input {
		display: block;
		width: 100%;
		max-width: 720px;
		margin: 0;
		flex: 1 1 min(720px, 100%);
		background: var(--panel-bg);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-lg);
		padding: 14px 22px;
		font-size: var(--font-size-md);
		color: var(--text-primary);
		outline: none;
		transition: border-color var(--motion-fast), background var(--motion-fast);
	}

	.search-input::placeholder {
		color: var(--text-tertiary);
	}

	.search-input:focus {
		border-color: var(--accent);
		background: var(--input-focus);
	}

	.editorial-link {
		flex: 0 0 auto;
		padding: var(--space-2) var(--space-3);
		border-radius: 999px;
		border: 1px solid var(--panel-border);
		background: var(--bg-hover);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		text-decoration: none;
		transition: background var(--motion-fast), border-color var(--motion-fast), color var(--motion-fast);
	}

	.editorial-link:hover,
	.editorial-link:focus-visible {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--text-primary);
		outline: none;
	}

	.hero {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(260px, 340px);
		gap: 20px;
		padding: 16px;
	}

	.hero--prompt {
		grid-template-columns: minmax(0, 1fr);
		width: min(100%, 720px);
		margin: 0 auto;
		animation: video-choice-prompt-in var(--motion-slow) cubic-bezier(0.22, 0.7, 0.2, 1) both;
	}

	.player-shell {
		min-width: 0;
		min-height: clamp(160px, 24vw, 420px);
		display: grid;
		align-items: center;
	}

	.hero--prompt .player-shell {
		min-height: 0;
	}

	.stage-anchor {
		width: 100%;
		display: grid;
		align-items: center;
	}

	/* When a video is playing, this is an empty 16/9 frame the fixed dock sits
	   on top of. Dark fill so the inline area reads as a video surface even in
	   the frame between rect measurement and the dock painting over it. */
	.stage-anchor.has-video {
		aspect-ratio: 16 / 9;
		border-radius: 8px;
		background: #030305;
	}

	.video-choice-prompt {
		justify-self: stretch;
		width: 100%;
		padding: var(--space-4) var(--space-5);
		border: 1px solid var(--accent-line);
		border-radius: var(--radius-md);
		background:
			linear-gradient(135deg, color-mix(in srgb, var(--accent-soft) 58%, transparent), transparent 76%),
			color-mix(in srgb, var(--panel-bg) 82%, transparent);
		box-shadow: 0 14px 34px color-mix(in srgb, var(--accent-glow) 40%, transparent);
		color: var(--text-primary);
	}

	.video-choice-prompt strong,
	.video-choice-prompt span {
		display: block;
	}

	.video-choice-prompt span {
		margin-top: var(--space-2);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	@keyframes video-choice-prompt-in {
		from {
			opacity: 0;
			transform: translateY(8px) scale(0.985);
			filter: blur(6px);
		}
		to {
			opacity: 1;
			transform: translateY(0) scale(1);
			filter: blur(0);
		}
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
		font-size: var(--font-size-2xl);
	}

	.meta-line {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		align-items: center;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.meta-link,
	.text-btn {
		color: var(--accent-strong);
		font-weight: var(--font-weight-bold);
	}

	.inline-error {
		color: var(--state-error);
		font-size: var(--font-size-sm);
	}

	.landing-row {
		width: 100%;
		max-width: 720px;
		margin: 0 auto;
		padding: 0 4px;
		display: grid;
		gap: var(--space-5);
	}

	.results-section {
		display: grid;
		gap: 14px;
	}

	.rail-block {
		display: grid;
		gap: var(--space-3);
	}

	.rail-header,
	.section-heading {
		display: flex;
		align-items: baseline;
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
		font-size: var(--font-size-md);
	}

	.chips {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-start;
		gap: 8px;
	}

	.hint-chip {
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		border-radius: 999px;
		padding: 7px 13px;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
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
		font-size: var(--font-size-xs);
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
