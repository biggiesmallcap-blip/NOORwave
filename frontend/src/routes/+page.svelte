<script lang="ts">
	import { onMount } from 'svelte';
	import type { Snapshot } from './$types';
	import {
		api,
		ApiError,
		type RSSFeedItem,
		type ReleaseItem,
		type HomePickTrack,
	} from '$lib/api/client';
	import TrendingShelf from '$lib/components/charts/TrendingShelf.svelte';
	import YourMixesShelf from '$lib/components/home/YourMixesShelf.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import TrendingCard from '$lib/components/TrendingCard.svelte';

	// Home page data
	let releases = $state<ReleaseItem[]>([]);
	let releasesNotConfigured = $state(false);
	let genrePicks = $state<HomePickTrack[]>([]);
	let articles = $state<RSSFeedItem[]>([]);
	let news = $state<RSSFeedItem[]>([]);

	// Loading states
	let error = $state<string | null>(null);
	let sectionsLoading = $state({
		releases: true,
		picks: true,
		articles: true,
		news: true
	});

	onMount(() => {
		void loadHome();
	});

	async function loadHome() {
		error = null;
		// Load all sections in parallel — each handles its own error state.
		loadReleases();
		loadPicks();
		loadArticles();
		loadNews();
	}

	// Phase 5B — back/forward state via SvelteKit snapshot
	export const snapshot: Snapshot<{ scrollY: number }> = {
		capture: () => ({
			scrollY: typeof window !== 'undefined' ? window.scrollY : 0
		}),
		restore: (saved) => {
			requestAnimationFrame(() => window.scrollTo({ top: saved.scrollY, behavior: 'auto' }));
		}
	};

	async function loadReleases() {
		sectionsLoading.releases = true;
		releasesNotConfigured = false;
		try {
			const data = await api.getHomeReleases();
			releases = data.releases ?? [];
		} catch (e) {
			if (e instanceof ApiError && e.status === 503) {
				// Backend signals "Last.fm not configured" via 503 so we can
				// render a connect prompt instead of a generic error.
				releasesNotConfigured = true;
				releases = [];
			} else {
				console.error('Failed to load releases:', e);
				releases = [];
			}
		} finally {
			sectionsLoading.releases = false;
		}
	}

	async function loadPicks() {
		sectionsLoading.picks = true;
		try {
			const data = await api.getHomePicks();
			genrePicks = data.genre_variety ?? [];
		} catch (e) {
			console.error('Failed to load picks:', e);
			genrePicks = [];
		} finally {
			sectionsLoading.picks = false;
		}
	}

	async function loadArticles() {
		sectionsLoading.articles = true;
		try {
			const data = await api.getHomeArticles();
			articles = data.articles ?? [];
		} catch (e) {
			console.error('Failed to load articles:', e);
			articles = [];
		} finally {
			sectionsLoading.articles = false;
		}
	}

	async function loadNews() {
		sectionsLoading.news = true;
		try {
			const data = await api.getHomeNews();
			news = data.news ?? [];
		} catch (e) {
			console.error('Failed to load news:', e);
			news = [];
		} finally {
			sectionsLoading.news = false;
		}
	}

	function formatDate(dateStr: string | null): string {
		if (!dateStr) return '';
		const date = new Date(dateStr);
		return date.toLocaleDateString(undefined, {
			month: 'short',
			day: 'numeric',
			year: 'numeric'
		});
	}

	function formatDuration(ms: number | null): string {
		if (!ms) return '';
		const minutes = Math.floor(ms / 60000);
		const seconds = Math.floor((ms % 60000) / 1000);
		return `${minutes}:${seconds.toString().padStart(2, '0')}`;
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
		<section class="page-header">
			<img class="page-wordmark" src="/wordmark-dark.svg" alt="NOORwave" />
		</section>
		<EmptyState title="NOOR is offline" copy={error}>
			{#snippet actions()}
				<button class="btn btn-glass" onclick={loadHome}>Try again</button>
			{/snippet}
		</EmptyState>
	{:else}
		<section class="page-header">
			<img class="page-wordmark" src="/wordmark-dark.svg" alt="NOORwave" />
		</section>

		<!-- Mobile quick-nav (hidden on desktop) -->
		<nav class="mobile-quick-nav" aria-label="Quick navigation">
			<a href="/library" class="quick-nav-tile">
				<span class="quick-nav-icon">♫</span>
				<span class="quick-nav-label">Library</span>
			</a>
			<a href="/discover" class="quick-nav-tile">
				<span class="quick-nav-icon">✦</span>
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

		<!-- Unified Trending shelf (Worldwide / Country / Genre / Tidal) -->
		<section class="discovery-section" data-section="trending">
			<TrendingShelf limit={12} />

			{#if genrePicks.length > 0}
				<div class="picks-subsection">
					<h3 class="subsection-title">Genre variety</h3>
					<div class="genre-pills">
						{#each genrePicks as pick, i (`${pick.id}-${i}`)}
							<div class="genre-pill glass-tile">
								<span class="genre-name">{pick.genre}</span>
								<span class="genre-track">{pick.title}</span>
							</div>
						{/each}
					</div>
				</div>
			{/if}
		</section>

		<!-- New Releases (now below Trending, sourced from Last.fm JSON API). -->
		<section class="discovery-section" data-section="new-releases">
			<div class="section-header">
				<div class="section-title-group">
					<p class="eyebrow">Last.fm</p>
					<h2>New releases</h2>
				</div>
				{#if sectionsLoading.releases}
					<span class="loading-indicator">Loading...</span>
				{/if}
			</div>

			{#if releases.length > 0}
				<div class="horizontal-scroll">
					{#each releases.slice(0, 12) as release (release.link || `${release.author}-${release.title}`)}
						<a class="release-card glass-tile" href={release.link} target="_blank" rel="noopener">
							{#if release.image_url}
								<img class="release-art" src={release.image_url} alt="" />
							{:else}
								<div class="release-art placeholder">💿</div>
							{/if}
							<div class="release-info">
								<h3 class="release-title">{release.title}</h3>
								{#if release.author}
									<p class="release-artist">{release.author}</p>
								{/if}
							</div>
						</a>
					{/each}
				</div>
			{:else if releasesNotConfigured}
				<EmptyState
					title="Connect Last.fm to see new releases"
					copy="Last.fm powers the new-releases shelf. Add your API key in Settings → Sources → Last.fm."
				/>
			{:else if !sectionsLoading.releases}
				<EmptyState title="No new releases found" copy="Last.fm did not return any recent albums." />
			{/if}
		</section>

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

	.page-header {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 8px;
		padding-top: 4px;
	}

	/* Brand wordmark in the page header — replaces the old title + status
	   badges. Centered, sized to feel like a hero rather than a label. */
	.page-wordmark {
		width: clamp(320px, 42vw, 640px);
		height: auto;
		display: block;
		margin: 0 auto;
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
		font-size: 1.15rem;
		font-weight: 700;
		margin: 0;
	}

	.loading-indicator {
		font-size: 0.78rem;
		color: var(--text-muted);
		font-style: italic;
	}

	.trending-controls {
		display: flex;
		align-items: center;
		gap: 12px;
	}
	.chip-group {
		display: inline-flex;
		gap: 4px;
		padding: 2px;
		background: rgba(255, 255, 255, 0.04);
		border-radius: 999px;
	}
	.chip {
		background: transparent;
		border: none;
		color: var(--text-muted, #888);
		font: inherit;
		font-size: 0.78rem;
		font-weight: 500;
		padding: 4px 10px;
		border-radius: 999px;
		cursor: pointer;
		transition: background 0.15s ease, color 0.15s ease;
	}
	.chip:hover {
		color: var(--text, #fff);
	}
	.chip.active {
		background: rgba(255, 255, 255, 0.12);
		color: var(--text, #fff);
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

	/* Release cards */
	.release-card {
		flex: 0 0 200px;
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 14px;
		text-decoration: none;
		color: inherit;
		transition: transform 0.2s ease, box-shadow 0.2s ease;
		scroll-snap-align: start;

		&:hover {
			transform: translateY(-4px);
			box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
		}
	}

	.release-art {
		width: 100%;
		aspect-ratio: 1;
		border-radius: 8px;
		object-fit: cover;
		background: var(--bg-surface);
	}

	.release-art.placeholder {
		width: 100%;
		aspect-ratio: 1;
		border-radius: 8px;
		background: var(--accent-soft);
		display: grid;
		place-items: center;
		font-size: 2.5rem;
	}

	.release-info {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.release-title {
		font-size: 0.88rem;
		font-weight: 600;
		margin: 0;
		display: -webkit-box;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}

	.release-artist {
		font-size: 0.78rem;
		color: var(--text-muted);
		margin: 0;
	}

	.release-source {
		font-size: 0.72rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.5px;
	}

	/* Picks grid */
	.picks-grid {
		display: flex;
		flex-direction: column;
		gap: 24px;
	}

	.picks-subsection {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.subsection-title {
		font-size: 0.95rem;
		font-weight: 600;
		color: var(--text-secondary);
		margin: 0;
	}

	.track-list {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
		gap: 12px;
	}

	.trending-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
		gap: 14px;
	}

	@media (max-width: 720px) {
		.trending-grid {
			grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
			gap: 10px;
		}
	}

	.track-row {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 12px;
		cursor: pointer;
		transition: transform 0.18s ease, box-shadow 0.18s ease, background 0.18s ease;

		&:hover {
			transform: translateY(-3px);
			box-shadow: 0 6px 20px rgba(0, 0, 0, 0.35);
			background: var(--bg-hover);
		}

		&:active {
			transform: translateY(-1px);
			box-shadow: 0 2px 8px rgba(0, 0, 0, 0.25);
		}
	}

	.track-art {
		width: 48px;
		height: 48px;
		border-radius: 6px;
		object-fit: cover;
		flex-shrink: 0;
		background: var(--bg-surface);
	}

	.track-art.placeholder {
		width: 48px;
		height: 48px;
		border-radius: 6px;
		background: var(--accent-soft);
		display: grid;
		place-items: center;
		color: var(--accent-strong);
		font-size: 1.2rem;
		flex-shrink: 0;
	}

	.track-meta {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.track-title {
		font-size: 0.88rem;
		font-weight: 600;
		margin: 0;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.track-artist {
		font-size: 0.78rem;
		color: var(--text-muted);
	}

	.track-stats {
		display: flex;
		gap: 12px;
		flex-shrink: 0;
	}

	.stat {
		font-size: 0.72rem;
		color: var(--text-muted);
		font-weight: 600;
	}

	.genre-pills {
		display: flex;
		flex-wrap: wrap;
		gap: 10px;
	}

	.genre-pill {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 10px 14px;
		border-radius: 8px;
	}

	.genre-name {
		font-size: 0.72rem;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.5px;
		color: var(--accent);
	}

	.genre-track {
		font-size: 0.82rem;
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 200px;
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
		font-size: 0.95rem;
		font-weight: 700;
		margin: 0;
		display: -webkit-box;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}

	.article-desc {
		font-size: 0.82rem;
		color: var(--text-muted);
		margin: 0;
		display: -webkit-box;
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
		font-size: 0.72rem;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.5px;
	}

	.article-date {
		font-size: 0.72rem;
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
		font-size: 0.95rem;
		font-weight: 700;
		margin: 0;
		display: -webkit-box;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}

	.news-desc {
		font-size: 0.82rem;
		color: var(--text-muted);
		margin: 0;
		display: -webkit-box;
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
		font-size: 0.72rem;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.5px;
	}

	.news-date {
		font-size: 0.72rem;
		color: var(--text-muted);
	}

	/* ── Mobile quick-nav (hidden on desktop) ── */
	.mobile-quick-nav {
		display: none;
	}

	/* Responsive */
	@media (max-width: 1180px) {
		.home-page { gap: var(--space-4); }
		.page-header { padding-top: 0; }

		/* System badges and now-playing bar are shown in mobile chrome */
		.system-badges { display: none; }
		.now-playing-bar { display: none; }

		.discovery-section { gap: 12px; }
		.section-title-group h2 { font-size: 1rem; }

		.track-list {
			grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
		}

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
			font-size: 1.3rem;
			line-height: 1;
		}

		.quick-nav-label {
			font-size: 0.72rem;
			font-weight: 600;
			letter-spacing: 0.02em;
		}
	}

	@media (max-width: 640px) {
		.track-list {
			grid-template-columns: 1fr;
		}

		.news-grid {
			grid-template-columns: 1fr;
		}

		.release-card {
			flex: 0 0 150px;
		}

		.article-card {
			flex: 0 0 260px;
		}

		.track-art {
			width: 42px;
			height: 42px;
		}

		.genre-pills {
			flex-wrap: nowrap;
			overflow-x: auto;
			padding-bottom: 4px;
		}

		.genre-pill {
			flex-shrink: 0;
		}
	}
</style>
