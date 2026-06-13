<script lang="ts">
	import { onMount } from 'svelte';
	import type { Unsubscriber } from 'svelte/store';
	import type { Snapshot } from './$types';
	import { captureScroll, restoreScroll } from '$lib/navigation/scroll';
	import {
		type RSSFeedItem,
	} from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import YourMixesShelf from '$lib/components/home/YourMixesShelf.svelte';
	import PersonalRadioShelf from '$lib/components/home/PersonalRadioShelf.svelte';
	import HomeRecommendationsShelf from '$lib/components/home/HomeRecommendationsShelf.svelte';
	import HomeMoodsRail from '$lib/components/home/HomeMoodsRail.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';

	// Home page data
	let articles = $state<RSSFeedItem[]>([]);
	let news = $state<RSSFeedItem[]>([]);

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

<div class="page-shell home-page animate-in">
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


		<!-- Your Mixes (TIDAL) — replaces the prime above-Trending slot. -->
		<YourMixesShelf />

		<!-- Personal Radio Stations (TIDAL) -->
		<PersonalRadioShelf />

		<!-- Provider recommendations load independently from profile integrations. -->
		<HomeRecommendationsShelf />

		<!-- Moods preview rail. Pulls the first chunk of categories from
		     /api/tidal/moods and links each tile to /moods/[slug]. Full
		     listing lives at /moods. -->
		<HomeMoodsRail />

		<!-- Weekly Articles Section -->
		<section class="discovery-section">
			<div class="section-header">
				<div class="section-title-group">
					<p class="eyebrow">AllMusic</p>
					<h2>Weekly articles</h2>
				</div>
				{#if sectionsLoading.articles}
					<span class="loading-indicator">Loading...</span>
				{/if}
			</div>

			{#if articles.length > 0}
				<div class="horizontal-scroll">
					{#each articles.slice(0, 10) as article (article.link)}
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
					{/each}
				</div>
			{:else}
				<EmptyState title="No articles this week" copy="Check back later for fresh music content." />
			{/if}
		</section>

		<!-- Industry News Section -->
		<section class="discovery-section">
			<div class="section-header">
				<div class="section-title-group">
					<p class="eyebrow">Industry</p>
					<h2>Latest news</h2>
				</div>
				{#if sectionsLoading.news}
					<span class="loading-indicator">Loading...</span>
				{/if}
			</div>

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

	/* Discovery sections */
	.discovery-section {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}

	.section-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
	}

	.section-title-group {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.section-title-group h2 {
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		margin: 0;
	}

	.loading-indicator {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
		font-style: italic;
	}

	/* Horizontal scroll */
	.horizontal-scroll {
		display: flex;
		gap: 16px;
		overflow-x: auto;
		padding-bottom: 8px;
		scroll-snap-type: x mandatory;

		&::-webkit-scrollbar {
			height: 6px;
		}

		&::-webkit-scrollbar-track {
			background: var(--bg-surface);
			border-radius: 3px;
		}

		&::-webkit-scrollbar-thumb {
			background: var(--border-subtle);
			border-radius: 3px;
		}

		&::-webkit-scrollbar-thumb:hover {
			background: var(--text-muted);
		}
	}

	/* Article cards */
	.article-card {
		flex: 0 0 320px;
		padding: 18px;
		text-decoration: none;
		color: inherit;
		transition: transform 0.2s ease, box-shadow 0.2s ease;
		scroll-snap-align: start;

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
		gap: 16px;
	}

	.news-card {
		padding: 18px;
		text-decoration: none;
		color: inherit;
		transition: transform 0.2s ease, box-shadow 0.2s ease;

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

		.discovery-section { gap: 12px; }
		.section-title-group h2 { font-size: var(--font-size-md); }

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

		.article-card {
			flex: 0 0 260px;
		}
	}
</style>
