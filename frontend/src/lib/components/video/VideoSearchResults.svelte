<script lang="ts">
	import { onDestroy } from 'svelte';
	import { api, ApiError, type TidalSearchVideo } from '$lib/api/client';
	import VideoCard from '$lib/components/video/VideoCard.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import { assertOnline } from '$lib/stores/player';
	import { showToast } from '$lib/stores/toast';
	import { readPersistedJson, removePersisted, writePersisted } from '$lib/stores/persisted';
	import { playVideo, videoSession } from '$lib/stores/video_session';
	import { videoSectionQuery } from '$lib/video/section';

	// TIDAL video search for the whole video section. The layout shows this in
	// place of the current page while the search field has text; picking a
	// result plays it and the layout opens the watch page.

	const PAGE_SIZE = 40;
	const RECENT_KEY = 'noor_recent_video_searches';
	const RECENT_MAX = 8;

	let videos = $state<TidalSearchVideo[]>([]);
	let loadingSearch = $state(false);
	let loadingMore = $state(false);
	let error = $state<string | null>(null);
	let offset = $state(0);
	let hasMore = $state(false);
	let lastQuery = $state('');
	let sentinel = $state<HTMLDivElement | null>(null);
	let recent = $state<string[]>(loadRecent());
	let debounceTimer: ReturnType<typeof setTimeout> | null = null;
	let searchAbort: AbortController | null = null;
	let loadMoreSeq = 0;

	let query = $derived($videoSectionQuery);

	function loadRecent(): string[] {
		const stored = readPersistedJson<unknown>(RECENT_KEY, []);
		if (!Array.isArray(stored)) return [];
		return stored.filter((item): item is string => typeof item === 'string').slice(0, RECENT_MAX);
	}

	function pushRecent(value: string) {
		const trimmed = value.trim();
		if (!trimmed) return;
		recent = [trimmed, ...recent.filter((item) => item.toLowerCase() !== trimmed.toLowerCase())].slice(0, RECENT_MAX);
		writePersisted(RECENT_KEY, JSON.stringify(recent));
	}

	function clearRecent() {
		recent = [];
		removePersisted(RECENT_KEY);
	}

	function normalizeError(errorValue: unknown, fallback: string): string {
		if (errorValue instanceof ApiError) return errorValue.message;
		if (errorValue instanceof Error) return errorValue.message;
		return fallback;
	}

	async function runSearch(nextQuery: string) {
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
		} catch (err) {
			if (controller.signal.aborted || (err as Error)?.name === 'AbortError') return;
			error = normalizeError(err, 'Video search failed.');
		} finally {
			if (searchAbort === controller) searchAbort = null;
			if (!controller.signal.aborted) loadingSearch = false;
		}
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

	async function pick(video: TidalSearchVideo) {
		if (!assertOnline()) {
			showToast('Server is reconnecting.', 'error', 3200);
			return;
		}
		const ok = await playVideo(video, {
			queue: videos,
			source: 'search',
			sourceLabel: lastQuery || null,
			autoplay: $videoSession.autoplay,
		});
		if (!ok) showToast($videoSession.error ?? 'This video could not be loaded.', 'error', 3200);
	}

	// Debounced search on every edit of the shared field.
	$effect(() => {
		const q = query;
		if (debounceTimer) clearTimeout(debounceTimer);
		loadMoreSeq += 1;
		loadingMore = false;
		debounceTimer = setTimeout(() => void runSearch(q), 250);
	});

	$effect(() => {
		if (!sentinel) return;
		const observer = new IntersectionObserver((entries) => {
			if (entries.some((entry) => entry.isIntersecting)) void loadMore();
		}, { rootMargin: '480px 0px' });
		observer.observe(sentinel);
		return () => observer.disconnect();
	});

	onDestroy(() => {
		if (debounceTimer) clearTimeout(debounceTimer);
		searchAbort?.abort();
		loadMoreSeq += 1;
	});
</script>

<div class="video-search-results">
	{#if recent.length > 0}
		<div class="recent-inline">
			<span class="eyebrow">Recent</span>
			<div class="chips">
				{#each recent as item (item)}
					<button type="button" class="hint-chip" onclick={() => videoSectionQuery.set(item)}>{item}</button>
				{/each}
			</div>
			<button type="button" class="text-btn" onclick={clearRecent}>Clear</button>
		</div>
	{/if}

	{#if loadingSearch && videos.length === 0}
		<div class="status-wrap"><Skeleton rows={4} label="Searching videos" /></div>
	{:else if error && videos.length === 0}
		<EmptyState title="Video search failed" copy={error} />
	{:else if !loadingSearch && lastQuery && videos.length === 0}
		<EmptyState title="No videos found" copy="Try a broader artist, song, or live-session search." />
	{/if}

	{#if videos.length > 0}
		<section class="results-section" aria-label="Video search results">
			<div class="section-heading">
				<p class="eyebrow">Results</p>
				<h2>{lastQuery}</h2>
			</div>
			<div class="video-grid">
				{#each videos as video (video.tidal_id)}
					<VideoCard {video} onSelect={(item) => !('id' in item) && void pick(item)} />
				{/each}
			</div>
			<div bind:this={sentinel} class="infinite-sentinel" aria-hidden="true">
				{#if loadingMore}<span>Loading more...</span>{/if}
			</div>
		</section>
	{/if}
</div>

<style>
	.video-search-results {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: 24px;
		animation: results-in var(--motion-base) both;
	}

	@keyframes results-in {
		from { opacity: 0; transform: translateY(6px); }
		to { opacity: 1; transform: none; }
	}

	.recent-inline {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-3);
		flex-wrap: wrap;
	}

	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.hint-chip {
		display: inline-flex;
		align-items: center;
		height: var(--control-h);
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		border-radius: 999px;
		padding: 0 14px;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
	}

	.hint-chip:hover {
		background: var(--accent-soft);
		color: var(--accent-strong);
		border-color: var(--accent-line);
	}

	.text-btn {
		color: var(--accent-strong);
		font-weight: var(--font-weight-bold);
	}

	.results-section {
		display: grid;
		gap: 14px;
	}

	.section-heading {
		display: flex;
		align-items: baseline;
		gap: 12px;
	}

	.section-heading h2 {
		margin: 0;
		color: var(--text-primary);
		font-size: var(--font-size-lg);
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

	@media (max-width: 620px) {
		.video-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
