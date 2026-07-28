<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import type { Unsubscriber } from 'svelte/store';
	import type { Snapshot } from './$types';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import { captureScroll, restoreScroll } from '$lib/navigation/scroll';
	import {
		type RSSFeedItem,
	} from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import YourMixesShelf from '$lib/components/home/YourMixesShelf.svelte';
	import PersonalRadioShelf from '$lib/components/home/PersonalRadioShelf.svelte';
	import HomeRecommendationsShelf from '$lib/components/home/HomeRecommendationsShelf.svelte';
	import HomeMoodsRail from '$lib/components/home/HomeMoodsRail.svelte';
	import HomeEditorialPreview from '$lib/components/home/HomeEditorialPreview.svelte';
	import DiscoverShelves from '$lib/components/search/DiscoverShelves.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import MediaRail from '$lib/components/ui/MediaRail.svelte';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';

	// Home page data
	let articles = $state<RSSFeedItem[]>([]);
	let news = $state<RSSFeedItem[]>([]);
	let homeQuery = $state('');

	function homeSearchKeydown(event: KeyboardEvent) {
		if (event.key !== 'Enter') return;
		const q = homeQuery.trim();
		if (!q) return;
		event.preventDefault();
		void goto(`/search?q=${encodeURIComponent(q)}`);
	}

	// Loading states
	let error = $state<string | null>(null);
	let sectionsLoading = $state({
		articles: true,
		news: true
	});
	let homeUnsubscribers: Unsubscriber[] = [];

	onMount(() => {
		const articlesQuery = cachedApi.homeArticlesQuery();
		const newsQuery = cachedApi.homeNewsQuery();
		homeUnsubscribers = [
			articlesQuery.subscribe((state) => {
				if (state.data) articles = state.data.articles ?? articles;
				sectionsLoading.articles = state.loading;
				if (state.error && !state.data) console.error('Failed to load articles:', state.error);
			}),
			newsQuery.subscribe((state) => {
				if (state.data) news = state.data.news ?? news;
				sectionsLoading.news = state.loading;
				if (state.error && !state.data) console.error('Failed to load news:', state.error);
			}),
		];
		return () => {
			for (const unsubscribe of homeUnsubscribers) unsubscribe();
			homeUnsubscribers = [];
		};
	});

	async function loadHome() {
		error = null;
		// Load all sections in parallel — each handles its own error state.
		const articlesQuery = cachedApi.homeArticlesQuery();
		const newsQuery = cachedApi.homeNewsQuery();
		await Promise.allSettled([articlesQuery.refresh(), newsQuery.refresh()]);
	}

	// Phase 5B — back/forward state via SvelteKit snapshot
	export const snapshot: Snapshot<{ scrollY: number }> = {
		capture: () => ({
			scrollY: captureScroll()
		}),
		restore: (saved) => {
			restoreScroll(saved.scrollY);
		}
	};

	function formatDate(dateStr: string | null): string {
		if (!dateStr) return '';
		const date = new Date(dateStr);
		return date.toLocaleDateString(undefined, {
			month: 'short',
			day: 'numeric',
			year: 'numeric'
		});
	}

	function getSourceColor(source: string): string {
		const colors: Record<string, string> = {
			'AllMusic': 'var(--accent)',
			'Billboard': '#ff6b6b',
			'NME': '#4ecdc4',
			'SPIN': '#ffe66d',
			'Pitchfork': '#95e1d3',
			'Rolling Stone': '#f38181',
			'Consequence': '#aa96da',
			'The Guardian Music': '#48bfe3'
		};
		return colors[source] || 'var(--text-muted)';
	}
</script>

<svelte:head>
	<title>NOOR — Home</title>
</svelte:head>

{#snippet articleCard(article: RSSFeedItem)}
	<a class="article-card glass-tile" href={article.link} target="_blank" rel="noopener">
		<div class="article-content">
			<h3 class="article-title">{article.title}</h3>
			{#if article.description}
				<p class="article-desc">{article.description}</p>
			{/if}
			<div class="article-footer">
				<span class="article-source" style="color: {getSourceColor(article.source)}">
					{article.source}
				</span>
				{#if article.published_at}
					<span class="article-date">{formatDate(article.published_at)}</span>
				{/if}
			</div>
		</div>
	</a>
{/snippet}

<!-- No page-level `animate-in` here. It fires once on mount, before any shelf
     has data, so everything that resolves later still pops in behind it. Each
     section eases itself in instead, staggered by its place in the stack -
     the same move Library made when it dropped its page-level translate. -->
<div class="page-shell home-page">
	{#if error}
		<EmptyState title="NOOR is offline" copy={error}>
			{#snippet actions()}
				<button class="btn btn-glass" onclick={loadHome}>Try again</button>
			{/snippet}
		</EmptyState>
	{:else}
		<!-- Mobile quick-nav (hidden on desktop) -->
		<nav class="mobile-quick-nav" aria-label="Quick navigation">
			<a href="/library" class="quick-nav-tile">
				<span class="quick-nav-icon">♫</span>
				<span class="quick-nav-label">Library</span>
			</a>
			<a href="/discoverspace" class="quick-nav-tile">
				<span class="quick-nav-icon">◈</span>
				<span class="quick-nav-label">Discover</span>
			</a>
			<a href="/genres" class="quick-nav-tile">
				<span class="quick-nav-icon">◈</span>
				<span class="quick-nav-label">Genres</span>
			</a>
			<a href="/playlists" class="quick-nav-tile">
				<span class="quick-nav-icon">☰</span>
				<span class="quick-nav-label">Playlists</span>
			</a>
		</nav>


		<!-- Home is the browse surface, so the search box lives here too, but the
		     searching itself stays on /search rather than being duplicated: that
		     route already owns the debounce, the provider fan-out, the filter
		     pills and the result ranking, and it already seeds itself from ?q=
		     on mount. This is a handoff, not a second implementation. -->
		<div class="home-search">
			<SearchField
				bind:value={homeQuery}
				variant="page"
				placeholder="Search Tidal's full catalogue"
				ariaLabel="Search"
				onkeydown={homeSearchKeydown}
			/>
		</div>

		<!-- `index` is the section's slot in the stack. It only spaces out the
		     entrance animation; nothing else reads it. YourMixesShelf owns two
		     sections (music + video), so the next index skips a slot, and
		     HomeRecommendationsShelf renders one section per provider shelf.

		     Order alternates heavy and light so no two murals or two grids sit
		     next to each other, which is what made the old page read as a stack
		     of slabs. -->

		<!-- Your Mixes (TIDAL) — replaces the prime above-Trending slot. -->
		<YourMixesShelf index={0} />

		<!-- Personal Radio Stations (TIDAL) -->
		<PersonalRadioShelf index={2} />

		<!-- Provider recommendations load independently from profile integrations. -->
		<HomeRecommendationsShelf index={3} />

		<!-- TIDAL's own editorial home modules (The Hits, New Tracks, New
		     Albums, Spotlighted Uploads, From our editors). These used to render
		     only in the /search empty state, which meant the app had two browse
		     surfaces and Home was the thinner one. -->
		<!-- Slots 6-10 are reserved: this renders one section per TIDAL module
		     and there are usually five, so later sections start at 11. The
		     indices only drive the entrance stagger, but two sections sharing a
		     slot land together and break the cascade. -->
		<DiscoverShelves index={6} quiet />

		<!-- Moods preview rail. Pulls the first chunk of categories from
		     /api/tidal/moods and links each tile to /moods/[slug]. Full
		     listing lives at /moods. -->
		<HomeMoodsRail index={11} />

		<!-- Previews of two editorial routes that already existed but had
		     nothing linking to them. Each hides itself when TIDAL returns no
		     modules for the page, which new-releases currently does. -->
		<HomeEditorialPreview
			pagePath="new-releases"
			title="New releases"
			href="/new-releases"
			index={12}
		/>
		<HomeEditorialPreview pagePath="hires" title="Hi-Res picks" href="/hires" index={13} />

		<!-- Weekly Articles Section -->
		<section class="discovery-section rise-in-shelf" style="--rise-index: 14">
			<SectionHeader eyebrow="AllMusic" title="Weekly articles" variant="charts" level={2}>
				{#snippet actions()}
					{#if sectionsLoading.articles}
						<span class="loading-indicator">Loading...</span>
					{/if}
				{/snippet}
			</SectionHeader>

			{#if articles.length > 0}
				<MediaRail
					items={articles.slice(0, 10)}
					card={articleCard}
					getKey={(a) => a.link}
					fluid
					density="wide"
					stagger
				/>
			{:else}
				<EmptyState title="No articles this week" copy="Check back later for fresh music content." />
			{/if}
		</section>

		<!-- Industry News Section -->
		<section class="discovery-section rise-in-shelf" style="--rise-index: 15">
			<SectionHeader eyebrow="Industry" title="Latest news" variant="charts" level={2}>
				{#snippet actions()}
					{#if sectionsLoading.news}
						<span class="loading-indicator">Loading...</span>
					{/if}
				{/snippet}
			</SectionHeader>

			{#if news.length > 0}
				<div class="news-grid">
					{#each news.slice(0, 15) as item (item.link)}
						<a class="news-card glass-tile" href={item.link} target="_blank" rel="noopener">
							<div class="news-content">
								<h3 class="news-title">{item.title}</h3>
								{#if item.description}
									<p class="news-desc">{item.description}</p>
								{/if}
								<div class="news-footer">
									<span class="news-source" style="color: {getSourceColor(item.source)}">
										{item.source}
									</span>
									{#if item.published_at}
										<span class="news-date">{formatDate(item.published_at)}</span>
									{/if}
								</div>
							</div>
						</a>
					{/each}
				</div>
			{:else}
				<EmptyState title="No news available" copy="Music news feeds are currently unavailable." />
			{/if}
		</section>
	{/if}
</div>

<style>
	.home-page {
		gap: var(--space-5);
		padding-bottom: 40px;
	}

	.home-search {
		width: min(100%, 720px);
	}

	/* Discovery sections. Two spacing values on this page and no others:
	   --space-5 between sections (from .page-shell) and --space-3 between a
	   section's header and its content. */
	.discovery-section {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.loading-indicator {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
		font-style: italic;
	}

	/* Article cards. Rail behaviour (scrolling, mask, fluid width) lives in
	   MediaRail; the card only describes itself. */
	.article-card {
		display: block;
		width: 100%;
		min-width: 0;
		height: 100%;
		box-sizing: border-box;
		padding: 18px;
		text-decoration: none;
		color: inherit;
		transition: transform var(--motion-base), box-shadow var(--motion-base);

		&:hover {
			transform: translateY(-4px);
			box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
		}
	}

	.article-content {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.article-title {
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-bold);
		margin: 0;
		display: -webkit-box;
		line-clamp: 2;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}

	.article-desc {
		font-size: var(--font-size-sm);
		color: var(--text-muted);
		margin: 0;
		display: -webkit-box;
		line-clamp: 3;
		-webkit-line-clamp: 3;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}

	.article-footer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		margin-top: 8px;
	}

	.article-source {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		text-transform: uppercase;
		letter-spacing: 0.5px;
	}

	.article-date {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
	}

	/* News grid */
	.news-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
		gap: var(--gap);
	}

	.news-card {
		padding: 18px;
		text-decoration: none;
		color: inherit;
		transition: transform var(--motion-base), box-shadow var(--motion-base);

		&:hover {
			transform: translateY(-4px);
			box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
		}
	}

	.news-content {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.news-title {
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-bold);
		margin: 0;
		display: -webkit-box;
		line-clamp: 2;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}

	.news-desc {
		font-size: var(--font-size-sm);
		color: var(--text-muted);
		margin: 0;
		display: -webkit-box;
		line-clamp: 3;
		-webkit-line-clamp: 3;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}

	.news-footer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		margin-top: 8px;
	}

	.news-source {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		text-transform: uppercase;
		letter-spacing: 0.5px;
	}

	.news-date {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
	}

	/* ── Mobile quick-nav (hidden on desktop) ── */
	.mobile-quick-nav {
		display: none;
	}

	/* Responsive */
	@media (max-width: 1180px) {
		.home-page { gap: var(--space-4); }

		.news-grid {
			grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
		}

		/* Show quick-nav tiles */
		.mobile-quick-nav {
			display: grid;
			grid-template-columns: repeat(4, 1fr);
			gap: 10px;
		}

		.quick-nav-tile {
			display: flex;
			flex-direction: column;
			align-items: center;
			justify-content: center;
			gap: 6px;
			padding: 14px 8px;
			border-radius: var(--radius-sm);
			background: color-mix(in srgb, var(--instrument-surface) 60%, transparent);
			border: 1px solid var(--border-subtle);
			text-decoration: none;
			color: var(--text-secondary);
			transition: background var(--motion-fast), color var(--motion-fast);
			-webkit-tap-highlight-color: transparent;
		}

		.quick-nav-tile:active {
			background: var(--accent-soft);
			color: var(--accent-strong);
		}

		.quick-nav-icon {
			font-size: var(--font-size-lg);
			line-height: 1;
		}

		.quick-nav-label {
			font-size: var(--font-size-xs);
			font-weight: var(--font-weight-semibold);
			letter-spacing: 0.02em;
		}
	}

	@media (max-width: 640px) {
		.news-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
