<script lang="ts">
	import { onMount } from 'svelte';
	import {
		api,
		getApiBase,
		type RSSFeedItem,
		type HomePickTrack
	} from '$lib/api/client';
	import { wsConnected } from '$lib/api/ws';
	import { currentTrack, isPlaying } from '$lib/stores/player';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';

	// Home page data
	let releases = $state<RSSFeedItem[]>([]);
	let picks = $state<HomePickTrack[]>([]);
	let genrePicks = $state<HomePickTrack[]>([]);
	let articles = $state<RSSFeedItem[]>([]);
	let news = $state<RSSFeedItem[]>([]);

	// Status data
	let status = $state<{ name: string; version: string; status: string } | null>(null);
	let tidalStatus = $state<'connected' | 'disconnected' | 'unknown'>('unknown');

	// Loading states
	let loading = $state(true);
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
		loading = false; // Show page immediately, sections load independently
		error = null;

		try {
			// Load status quickly
			status = await api.getStatus();

			// Load TIDAL status
			try {
				const tidalResponse = await fetch(`${getApiBase()}/api/tidal/status`);
				if (!tidalResponse.ok) throw new Error(`Server returned ${tidalResponse.status}`);
				const tidalData = await tidalResponse.json();
				tidalStatus = tidalData.connected ? 'connected' : 'disconnected';
			} catch {
				tidalStatus = 'unknown';
			}
		} catch {
			error = 'NOOR could not reach the local server.';
			loading = false;
			return;
		}

		// Load all sections in parallel (don't await, let them populate as they finish)
		loadReleases();
		loadPicks();
		loadArticles();
		loadNews();
	}

	async function loadReleases() {
		sectionsLoading.releases = true;
		try {
			const data = await api.getHomeReleases();
			releases = data.releases ?? [];
		} catch (e) {
			console.error('Failed to load releases:', e);
			releases = [];
		} finally {
			sectionsLoading.releases = false;
		}
	}

	async function loadPicks() {
		sectionsLoading.picks = true;
		try {
			const data = await api.getHomePicks();
			picks = data.top_picks ?? [];
			genrePicks = data.genre_variety ?? [];
		} catch (e) {
			console.error('Failed to load picks:', e);
			picks = [];
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
			<p class="eyebrow">NOOR</p>
			<h1>Your music discovery hub</h1>
		</section>
		<EmptyState title="NOOR is offline" copy={error}>
			{#snippet actions()}
				<button class="btn btn-glass" onclick={loadHome}>Try again</button>
			{/snippet}
		</EmptyState>
	{:else}
		<section class="page-header">
			<p class="eyebrow">NOOR</p>
			<h1>Your music discovery hub</h1>
			<div class="system-badges">
				{#if status}
					<StateBadge label={`Server v${status?.version}`} tone="success" />
				{:else}
					<StateBadge label="Loading..." tone="muted" />
				{/if}
				<StateBadge
					label={tidalStatus === 'connected' ? 'TIDAL connected' : tidalStatus === 'disconnected' ? 'TIDAL offline' : 'TIDAL unknown'}
					tone={tidalStatus === 'connected' ? 'active' : tidalStatus === 'disconnected' ? 'muted' : 'warning'}
				/>
				<StateBadge label={$wsConnected ? 'WS live' : 'WS offline'} tone={$wsConnected ? 'success' : 'muted'} />
			</div>
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

		{#if $currentTrack}
			<section class="glass-panel now-playing-bar">
				<div class="np-left">
					{#if $currentTrack.artwork_url}
						<img class="np-art" src={$currentTrack.artwork_url} alt="" />
					{:else}
						<div class="np-art placeholder">♫</div>
					{/if}
					<div class="np-meta">
						<p class="eyebrow">{$isPlaying ? 'Now playing' : 'Paused'}</p>
						<h3>{$currentTrack.title}</h3>
						<span>{$currentTrack.artist_name ?? 'Unknown artist'}</span>
					</div>
				</div>
			</section>
		{/if}

		<!-- New Releases Section -->
		<section class="discovery-section">
			<div class="section-header">
				<div class="section-title-group">
					<p class="eyebrow">Fresh from AllMusic</p>
					<h2>New Releases</h2>
				</div>
				{#if sectionsLoading.releases}
					<span class="loading-indicator">Loading...</span>
				{/if}
			</div>

			{#if releases.length > 0}
				<div class="horizontal-scroll">
					{#each releases.slice(0, 12) as release (release.link)}
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
								<span class="release-source" style="color: {getSourceColor(release.source)}">
									{release.source}
								</span>
							</div>
						</a>
					{/each}
				</div>
			{:else}
				<EmptyState title="No new releases found" copy="AllMusic feed is currently unavailable." />
			{/if}
		</section>

		<!-- Daily Picks Section -->
		<section class="discovery-section">
			<div class="section-header">
				<div class="section-title-group">
					<p class="eyebrow">Curated for you</p>
					<h2>Daily Picks</h2>
				</div>
				{#if sectionsLoading.picks}
					<span class="loading-indicator">Loading...</span>
				{/if}
			</div>

			{#if picks.length > 0 || genrePicks.length > 0}
				<div class="picks-grid">
					{#if picks.length > 0}
						<div class="picks-subsection">
							<h3 class="subsection-title">Top Picks</h3>
							<div class="track-list">
								{#each picks.slice(0, 8) as pick (pick.id)}
									<div class="track-row glass-tile">
										{#if pick.artwork_url}
											<img class="track-art" src={pick.artwork_url} alt="" />
										{:else}
											<div class="track-art placeholder">♫</div>
										{/if}
										<div class="track-meta">
											<p class="track-title">{pick.title}</p>
											<span class="track-artist">{pick.artist_name ?? 'Unknown artist'}</span>
										</div>
										<div class="track-stats">
											<span class="stat">{pick.play_count} plays</span>
											{#if pick.duration_ms}
												<span class="stat">{formatDuration(pick.duration_ms)}</span>
											{/if}
										</div>
									</div>
								{/each}
							</div>
						</div>
					{/if}

					{#if genrePicks.length > 0}
						<div class="picks-subsection">
							<h3 class="subsection-title">Genre Variety</h3>
							<div class="genre-pills">
								{#each genrePicks as pick (pick.id)}
									<div class="genre-pill glass-tile">
										<span class="genre-name">{pick.genre}</span>
										<span class="genre-track">{pick.title}</span>
									</div>
								{/each}
							</div>
						</div>
					{/if}
				</div>
			{:else}
				<EmptyState title="No daily picks yet" copy="Start listening to get personalized recommendations." />
			{/if}
		</section>

		<!-- Weekly Articles Section -->
		<section class="discovery-section">
			<div class="section-header">
				<div class="section-title-group">
					<p class="eyebrow">From AllMusic</p>
					<h2>Weekly Articles</h2>
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
					<p class="eyebrow">Music industry</p>
					<h2>Latest News</h2>
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
		gap: 8px;
		padding-top: 4px;
	}

	.page-header h1 {
		max-width: 16ch;
	}

	.system-badges {
		display: flex;
		flex-wrap: wrap;
		gap: 10px;
		margin-top: 8px;
	}

	/* Now playing bar */
	.now-playing-bar {
		padding: 16px 20px;
		display: flex;
		align-items: center;
		gap: 16px;
	}

	.np-left {
		display: flex;
		align-items: center;
		gap: 14px;
	}

	.np-art {
		width: 52px;
		height: 52px;
		border-radius: 8px;
		object-fit: cover;
		flex-shrink: 0;
	}

	.np-art.placeholder {
		background: var(--accent-soft);
		display: grid;
		place-items: center;
		color: var(--accent-strong);
		font-size: 1.2rem;
	}

	.np-meta {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.np-meta h3 {
		font-size: 0.95rem;
		font-weight: 700;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		margin: 0;
	}

	.np-meta span {
		font-size: 0.8rem;
		color: var(--text-muted);
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

	.track-row {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 12px;
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
