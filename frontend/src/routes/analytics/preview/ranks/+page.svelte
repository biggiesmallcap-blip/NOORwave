<script lang="ts">
	/**
	 * Dev-only preview for RankList — track / artist / genre kinds composed
	 * the way they'll sit on the rewritten analytics page.
	 */

	import RankList from '$lib/components/analytics/RankList.svelte';
	import fixture from '$lib/fixtures/analytics-signals.json';
	import type {
		AnalyticsTopArtist,
		AnalyticsTopTrack,
		AnalyticsGenreShare,
	} from '$lib/api/client';

	const signals = (fixture as { signals: any }).signals;
	const tracks = signals.top_tracks as AnalyticsTopTrack[];
	const artists = signals.top_artists as AnalyticsTopArtist[];
	const genres = signals.top_genres as AnalyticsGenreShare[];
</script>

{#if import.meta.env.DEV}
	<div class="preview">
		<header>
			<h1>Ranks — preview</h1>
			<p class="subtitle">
				<code>RankList</code> in three kinds — Top tracks, Top artists, Top genres.
				Right-click a track or artist row to confirm the universal context menu wires
				up correctly. Dev-only route.
			</p>
		</header>

		<div class="duo">
			<RankList kind="track" items={tracks} title="Top tracks" />
			<RankList kind="artist" items={artists} title="Top artists" />
		</div>

		<RankList kind="genre" items={genres} title="Top genres" limit={6} />

		<aside class="info">
			<h2>Source data</h2>
			<dl>
				<dt>Tracks</dt><dd>{tracks.length}</dd>
				<dt>Artists</dt><dd>{artists.length}</dd>
				<dt>Genres</dt><dd>{genres.length}</dd>
			</dl>
			<p class="hint">
				Click a track row → <code>playTrackNow</code>. Click an artist row →
				<code>goto(/artists/:id)</code>. Right-click → <code>buildTrackMenu</code> /
				<code>buildArtistMenu</code> from <code>$lib/player/*_menu</code>. Genre rows
				are non-interactive (genres aren't asset references).
			</p>
		</aside>
	</div>
{:else}
	<div class="not-found">
		<h1>404</h1>
		<p>This route is dev-only.</p>
	</div>
{/if}

<style>
	.preview {
		max-width: 1280px;
		margin: 0 auto;
		padding: var(--space-5) var(--space-5) var(--space-7);
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}

	header h1 {
		font-family: var(--font-display);
		font-size: 1.6rem;
		font-weight: 600;
		margin: 0 0 var(--space-1);
	}

	.subtitle {
		font-family: var(--font-body);
		color: var(--text-secondary);
		margin: 0;
	}

	.duo {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--space-4);
	}

	@media (max-width: 900px) {
		.duo {
			grid-template-columns: minmax(0, 1fr);
		}
	}

	.info {
		display: grid;
		grid-template-columns: 1fr;
		gap: var(--space-3);
		padding: var(--space-4);
		background: var(--bg-elevated);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
	}

	.info h2 {
		font-family: var(--font-mono);
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--text-tertiary);
		margin: 0;
	}

	.info dl {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: var(--space-2) var(--space-4);
		margin: 0;
		font-family: var(--font-mono);
		font-size: 0.78rem;
	}

	.info dt {
		color: var(--text-tertiary);
	}

	.info dd {
		margin: 0;
		color: var(--text-primary);
	}

	.info .hint {
		margin: 0;
		font-family: var(--font-body);
		color: var(--text-secondary);
		font-size: 0.85rem;
	}

	.not-found {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		min-height: 60vh;
		gap: var(--space-3);
	}
</style>
