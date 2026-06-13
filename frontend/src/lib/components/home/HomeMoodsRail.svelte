<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type TidalMoodCategory } from '$lib/api/client';
	import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { goto } from '$app/navigation';
	import {
		claimMoodThumbnailRefresh,
		getCachedMoodCategories,
		moodCategoriesNeedThumbnails,
		putCachedMoodCategories,
	} from '$lib/stores/tidal-moods-cache';

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

{#if categories.length > 0 || loading}
	<section bind:this={sectionEl} class="moods-rail" data-section="moods">
		<div class="header">
			<div class="title-group">
				<p class="eyebrow">TIDAL</p>
				<h2>Moods &amp; Activities</h2>
			</div>
			<a class="view-all" href="/moods">View all -&gt;</a>
		</div>
		{#if categories.length > 0}
			<div class="rail" use:wheelToHorizontal>
				{#each categories as c (c.slug)}
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
								fallbackText="~"
							/>
						</div>
						<p class="card-title">{c.title}</p>
					</a>
				{/each}
			</div>
		{:else}
			<div class="rail" aria-hidden="true">
				{#each [0, 1, 2, 3, 4, 5, 6, 7] as i (i)}
					<div class="card skeleton">
						<div class="art-wrap"><div class="art skeleton-art"></div></div>
						<div class="skeleton-line"></div>
					</div>
				{/each}
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
	.rail {
		display: flex;
		gap: var(--gap-sm);
		overflow-x: auto;
		padding-bottom: var(--space-2);
		scroll-snap-type: x mandatory;
		mask-image: linear-gradient(to right, transparent 0, black 16px, black calc(100% - 32px), transparent 100%);
		-webkit-mask-image: linear-gradient(to right, transparent 0, black 16px, black calc(100% - 32px), transparent 100%);
	}
	.rail::-webkit-scrollbar { height: 6px; }
	.rail::-webkit-scrollbar-track { background: var(--bg-surface); border-radius: var(--radius-xs); }
	.rail::-webkit-scrollbar-thumb { background: var(--border-subtle); border-radius: var(--radius-xs); }
	.rail::-webkit-scrollbar-thumb:hover { background: var(--text-muted); }
	.card {
		flex: 0 0 180px;
		width: 180px;
		min-width: 180px;
		max-width: 180px;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		background: none;
		border: 1px solid transparent;
		padding: var(--space-2);
		border-radius: var(--radius-md);
		text-decoration: none;
		color: inherit;
		cursor: pointer;
		transition: background var(--motion-base), border-color var(--motion-base);
		box-sizing: border-box;
		scroll-snap-align: start;
	}
	.card:hover, .card:focus-visible { background: var(--bg-hover); border-color: var(--panel-border); outline: none; }
	.card:focus-visible { border-color: var(--accent-line); }
	.art-wrap { position: relative; aspect-ratio: 1 / 1; width: 100%; border-radius: var(--radius-sm); overflow: hidden; background: var(--bg-hover); }
	:global(.mood-art) { width: 100%; height: 100%; object-fit: cover; display: block; transition: transform var(--motion-base); }
	:global(.mood-art.fallback) { display: flex; align-items: center; justify-content: center; background: var(--bg-hover); }
	:global(.mood-art.fallback span) { font-size: var(--font-size-4xl); color: var(--text-muted); }
	.card:hover :global(.mood-art) { transform: scale(1.05); }
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
