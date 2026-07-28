<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type TidalMoodCategory } from '$lib/api/client';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import MediaRail from '$lib/components/ui/MediaRail.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { goto } from '$app/navigation';
	import {
		claimMoodThumbnailRefresh,
		getCachedMoodCategories,
		moodCategoriesNeedThumbnails,
		putCachedMoodCategories,
	} from '$lib/stores/tidal-moods-cache';

	// Position in the home stack; stagger only. See YourMixesShelf.
	let { index = 0 }: { index?: number } = $props();

	const PREVIEW_LIMIT = 8;
	const LOAD_ARM_DELAY_MS = 0;
	const FALLBACK_LOAD_DELAY_MS = 3000;
	const THUMBNAIL_REFRESH_DELAY_MS = 1800;
	// The server fills mood thumbnails via a background probe that can land a few
	// seconds after the first response, so poll a bounded number of times rather
	// than giving up after one try and leaving the tiles as "~" forever.
	const THUMBNAIL_RETRY_INTERVAL_MS = 2500;
	const MAX_THUMBNAIL_ATTEMPTS = 6;

	// Sync-read the shared moods cache on script init so a second visit
	// within the 6h TTL renders instantly without a network round-trip.
	// /moods landing uses the same cache, so visiting either page warms it
	// for the other.
	const cachedOnMount = getCachedMoodCategories();
	let categories = $state<TidalMoodCategory[]>(
		cachedOnMount ? cachedOnMount.slice(0, PREVIEW_LIMIT) : [],
	);
	let loading = $state(!cachedOnMount);
	let errored = $state(false);
	let sectionEl = $state<HTMLElement | null>(null);
	let inFlight = false;
	let loadSeq = 0;
	let thumbnailRefreshTimer: ReturnType<typeof setTimeout> | null = null;
	let thumbnailAttempts = 0;

	async function loadMoods() {
		if (inFlight) return;
		const seq = ++loadSeq;
		inFlight = true;
		try {
			// Raw client, not cachedApi: the cachedApi layer persists the moods
			// response to localStorage for days and serves it stale, so a cold-start
			// thumbnail-less fallback would stick forever. The component's own
			// tidal-moods-cache handles session caching and knows to refresh when
			// thumbnails are missing.
			const data = await api.getTidalMoods();
			if (seq !== loadSeq) return;
			const all = data.categories ?? [];
			if (all.length > 0) putCachedMoodCategories(all);
			categories = all.slice(0, PREVIEW_LIMIT);
			scheduleThumbnailRefresh(all);
		} catch {
			if (seq !== loadSeq) return;
			errored = true;
		} finally {
			if (seq === loadSeq) {
				loading = false;
				inFlight = false;
			}
		}
	}

	onMount(() => {
		if (cachedOnMount && moodCategoriesNeedThumbnails(cachedOnMount)) {
			scheduleThumbnailRefresh(cachedOnMount);
			return () => {
				loadSeq += 1;
				clearThumbnailRefresh();
			};
		}
		if (cachedOnMount) {
			return () => { loadSeq += 1; };
		}

		let observer: IntersectionObserver | null = null;
		let fallbackTimer: ReturnType<typeof setTimeout> | null = null;
		const armTimer = setTimeout(() => {
			if (typeof IntersectionObserver === 'undefined' || !sectionEl) {
				void loadMoods();
				return;
			}

			observer = new IntersectionObserver(
				(entries) => {
					if (!entries.some((entry) => entry.isIntersecting)) return;
					observer?.disconnect();
					observer = null;
					void loadMoods();
				},
				{ rootMargin: '240px 0px' },
			);
			observer.observe(sectionEl);
			fallbackTimer = setTimeout(() => void loadMoods(), FALLBACK_LOAD_DELAY_MS);
		}, LOAD_ARM_DELAY_MS);

		return () => {
			loadSeq += 1;
			clearTimeout(armTimer);
			if (fallbackTimer) clearTimeout(fallbackTimer);
			clearThumbnailRefresh();
			observer?.disconnect();
		};
	});

	function clearThumbnailRefresh() {
		if (!thumbnailRefreshTimer) return;
		clearTimeout(thumbnailRefreshTimer);
		thumbnailRefreshTimer = null;
	}

	function scheduleThumbnailRefresh(nextCategories: TidalMoodCategory[]) {
		clearThumbnailRefresh();
		if (!moodCategoriesNeedThumbnails(nextCategories)) {
			thumbnailAttempts = 0;
			return;
		}
		if (thumbnailAttempts >= MAX_THUMBNAIL_ATTEMPTS) return;
		const delay =
			thumbnailAttempts === 0 ? THUMBNAIL_REFRESH_DELAY_MS : THUMBNAIL_RETRY_INTERVAL_MS;
		thumbnailRefreshTimer = setTimeout(() => {
			thumbnailRefreshTimer = null;
			const firstAttempt = thumbnailAttempts === 0;
			thumbnailAttempts += 1;
			// The first refresh honours the shared cross-surface throttle; if another
			// surface already claimed it, re-arm so the bounded follow-up polls still
			// pick up its result. loadMoods re-arms this poll on completion, so it
			// stops once thumbnails arrive or the attempt cap is hit.
			if (firstAttempt && !claimMoodThumbnailRefresh(nextCategories)) {
				scheduleThumbnailRefresh(nextCategories);
				return;
			}
			void loadMoods();
		}, delay);
	}

	function menu(slug: string, title: string) {
		return [{ label: `Open ${title}`, onSelect: () => void goto(`/moods/${slug}`) }];
	}
</script>

{#snippet moodCard(c: TidalMoodCategory)}
	<a
		class="card"
		href={`/moods/${c.slug}`}
		oncontextmenu={(e) => { e.preventDefault(); e.stopPropagation(); openContextMenu(e, menu(c.slug, c.title), c.title); }}
	>
		<div class="art-wrap">
			<ArtworkImage
				className="mood-art"
				src={c.thumbnail}
				alt={c.title}
				size={320}
				tint={true}
				fallbackText="~"
			/>
		</div>
		<p class="card-title">{c.title}</p>
	</a>
{/snippet}

{#snippet skeletonCard()}
	<div class="card skeleton">
		<div class="art-wrap"><div class="art skeleton-art"></div></div>
		<div class="skeleton-line"></div>
	</div>
{/snippet}

{#if categories.length > 0 || loading}
	<section bind:this={sectionEl} class="moods-rail rise-in-shelf" data-section="moods" style={`--rise-index: ${index}`}>
		<div class="header">
			<div class="title-group">
				<p class="eyebrow">TIDAL</p>
				<h2>Moods &amp; Activities</h2>
			</div>
			<a class="view-all" href="/moods">View all -&gt;</a>
		</div>
		{#if categories.length > 0}
			<MediaRail
				items={categories}
				card={moodCard}
				getKey={(c) => c.slug}
				fluid
				stagger
			/>
		{:else}
			<div aria-hidden="true">
				<MediaRail
					items={[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]}
					card={skeletonCard}
					getKey={(i) => i}
					fluid
				/>
			</div>
		{/if}
	</section>
{/if}
{#if errored && categories.length === 0}
	<!-- TIDAL disconnected or fetch failed: hide the rail entirely. -->
{/if}

<style>
	.moods-rail { display: flex; flex-direction: column; gap: var(--gap); }
	.header { display: flex; align-items: center; justify-content: space-between; gap: var(--gap); }
	.title-group { display: flex; flex-direction: column; gap: var(--space-1); }
	.title-group h2 { font-size: var(--font-size-lg); font-weight: var(--font-weight-bold); margin: 0; }
	.eyebrow { font-size: var(--font-size-xs); letter-spacing: 0.08em; text-transform: uppercase; color: var(--text-secondary); margin: 0; font-weight: var(--font-weight-bold); }
	.view-all { font-size: var(--font-size-xs); font-weight: var(--font-weight-semibold); color: var(--text-secondary); text-decoration: none; transition: color var(--motion-fast); }
	.view-all:hover, .view-all:focus-visible { color: var(--text-primary); outline: none; }
	/* Rail behaviour lives in MediaRail; this card only describes itself. */
	.card {
		width: 100%;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		background: none;
		border: 0;
		padding: 0;
		border-radius: var(--radius-md);
		text-decoration: none;
		color: inherit;
		cursor: pointer;
		transition: transform var(--motion-base);
		box-sizing: border-box;
	}
	.card:hover { transform: translateY(-4px); }
	.card:focus-visible { outline: 2px solid var(--accent); outline-offset: 4px; }
	.art-wrap { position: relative; aspect-ratio: 1 / 1; width: 100%; border-radius: var(--radius-md); overflow: hidden; background: var(--bg-raised); box-shadow: 0 2px 8px rgba(0, 0, 0, 0.22); transition: box-shadow var(--motion-base); }
	.card:hover .art-wrap { box-shadow: 0 12px 26px -6px rgba(0, 0, 0, 0.5); }
	:global(.mood-art) { width: 100%; height: 100%; object-fit: cover; display: block; }
	:global(.mood-art.fallback) { display: flex; align-items: center; justify-content: center; }
	:global(.mood-art.fallback span) { font-size: var(--font-size-4xl); color: rgba(255, 255, 255, 0.92); }
	.card-title { margin: 0; font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; line-height: var(--line-height-snug); }

	.card.skeleton { cursor: default; pointer-events: none; }
	.skeleton-art {
		width: 100%;
		height: 100%;
		background: linear-gradient(110deg, rgba(255,255,255,0.04) 30%, rgba(255,255,255,0.08) 50%, rgba(255,255,255,0.04) 70%);
		background-size: 200% 100%;
		animation: home-moods-shimmer 1.4s linear infinite;
	}
	.skeleton-line {
		height: 0.7rem;
		width: 70%;
		border-radius: var(--radius-xs);
		background: rgba(255,255,255,0.08);
	}
	@keyframes home-moods-shimmer {
		0%   { background-position: 200% 0; }
		100% { background-position: -200% 0; }
	}
</style>
