<script lang="ts">
	/**
	 * RankList — restyled top-N list. Three kinds:
	 *   - "track":  index · title (artist · plays right-aligned). Click → playTrackNow.
	 *   - "artist": index · name (Hh Mm right-aligned). Click → goto(/artists/:id).
	 *   - "genre":  index · name (pct% right-aligned). No interaction.
	 *
	 * Numbered mono index. 1px row dividers. Hover bg rgba(255,255,255,0.04).
	 * Right-click on track → buildTrackMenu. Right-click on artist → buildArtistMenu.
	 *
	 * Spec: C:\Users\Felix\.claude\plans\lets-revision-analytics-stats-crystalline-melody.md
	 */

	import { goto } from '$app/navigation';
	import type {
		AnalyticsTopArtist,
		AnalyticsTopTrack,
		AnalyticsGenreShare,
	} from '$lib/api/client';
	import { formatCount, formatDuration, formatPercent } from '$lib/utils/format';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTrackMenu, type MenuTrack } from '$lib/player/track_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { playTrackNow } from '$lib/stores/player';

	type Kind = 'track' | 'artist' | 'genre';

	interface Props {
		kind: Kind;
		items: AnalyticsTopTrack[] | AnalyticsTopArtist[] | AnalyticsGenreShare[];
		title: string;
		/** Number of rows to render. Default 5 (per spec). */
		limit?: number;
		/**
		 * Override hooks for the dev preview route — production code lets these
		 * fall through to the real store wiring.
		 */
		onplay?: (trackId: number) => void;
		onopenartist?: (artistId: number) => void;
	}

	let { kind, items, title, limit = 5, onplay, onopenartist }: Props = $props();

	const tracks = $derived(kind === 'track' ? (items as AnalyticsTopTrack[]).slice(0, limit) : []);
	const artists = $derived(
		kind === 'artist' ? (items as AnalyticsTopArtist[]).slice(0, limit) : [],
	);
	const genres = $derived(
		kind === 'genre' ? (items as AnalyticsGenreShare[]).slice(0, limit) : [],
	);

	function trackMenuTrack(t: AnalyticsTopTrack): MenuTrack {
		return {
			id: t.track_id,
			title: t.title,
			artist_name: t.artist_name,
			album_title: t.album_title,
		};
	}

	function activateTrack(t: AnalyticsTopTrack) {
		if (onplay) {
			onplay(t.track_id);
			return;
		}
		void playTrackNow(t.track_id);
	}

	function activateArtist(a: AnalyticsTopArtist) {
		if (onopenartist) {
			onopenartist(a.artist_id);
			return;
		}
		void goto(`/artists/${a.artist_id}`);
	}

	function onTrackContext(event: MouseEvent, t: AnalyticsTopTrack) {
		openContextMenu(event, buildTrackMenu(trackMenuTrack(t)));
	}

	function onArtistContext(event: MouseEvent, a: AnalyticsTopArtist) {
		openContextMenu(
			event,
			buildArtistMenu({ id: a.artist_id, name: a.artist_name, in_library: true }),
		);
	}

	function indexLabel(i: number): string {
		return String(i + 1).padStart(2, '0');
	}

	function genrePct(genre: AnalyticsGenreShare): string {
		return formatPercent(genre.share_of_window_listens ?? null, { decimals: 0 });
	}

	const isEmpty = $derived(
		(kind === 'track' && tracks.length === 0) ||
			(kind === 'artist' && artists.length === 0) ||
			(kind === 'genre' && genres.length === 0),
	);
</script>

<section class="rank glass" aria-label={title}>
	<header class="head">
		<span class="eyebrow">{title}</span>
	</header>

	{#if isEmpty}
		<p class="empty">No data in window.</p>
	{:else if kind === 'track'}
		<ol class="rows">
			{#each tracks as t, i (t.track_id)}
				<li class="row">
					<button
						type="button"
						class="hit"
						onclick={() => activateTrack(t)}
						oncontextmenu={(e) => onTrackContext(e, t)}
					>
						<span class="idx">{indexLabel(i)}</span>
						<span class="primary" title={t.title}>{t.title}</span>
						<span class="meta">
							<span class="artist">{t.artist_name ?? 'Unknown artist'}</span>
							<span class="dot">·</span>
							<span class="value">{formatCount(t.listens)}</span>
						</span>
					</button>
				</li>
			{/each}
		</ol>
	{:else if kind === 'artist'}
		<ol class="rows">
			{#each artists as a, i (a.artist_id)}
				<li class="row">
					<button
						type="button"
						class="hit"
						onclick={() => activateArtist(a)}
						oncontextmenu={(e) => onArtistContext(e, a)}
					>
						<span class="idx">{indexLabel(i)}</span>
						<span class="primary" title={a.artist_name}>{a.artist_name}</span>
						<span class="value">{formatDuration(a.total_listened_ms)}</span>
					</button>
				</li>
			{/each}
		</ol>
	{:else}
		<ol class="rows">
			{#each genres as g, i (g.genre_name)}
				<li class="row genre-row">
					<span class="idx">{indexLabel(i)}</span>
					<span class="primary" title={g.genre_name}>{g.genre_name}</span>
					<span class="value">{genrePct(g)}</span>
				</li>
			{/each}
		</ol>
	{/if}
</section>

<style>
	.rank {
		padding: var(--space-4);
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		min-height: 0;
	}

	.head {
		display: flex;
		align-items: baseline;
		gap: var(--space-3);
	}

	.eyebrow {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-tertiary);
	}

	.empty {
		font-family: var(--font-body);
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		margin: 0;
	}

	.rows {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
	}

	.row {
		border-top: 1px solid var(--border-subtle);
	}

	.row:first-child {
		border-top: none;
	}

	.hit {
		all: unset;
		display: grid;
		grid-template-columns: auto 1fr auto;
		align-items: baseline;
		gap: var(--space-3);
		width: 100%;
		padding: var(--space-2) var(--space-1);
		cursor: pointer;
		transition: background-color 120ms ease;
		box-sizing: border-box;
	}

	.hit:hover,
	.hit:focus-visible {
		background: rgba(255, 255, 255, 0.04);
	}

	.hit:focus-visible {
		outline: 1px solid var(--border-strong);
		outline-offset: -1px;
	}

	.genre-row {
		display: grid;
		grid-template-columns: auto 1fr auto;
		align-items: baseline;
		gap: var(--space-3);
		padding: var(--space-2) var(--space-1);
	}

	.idx {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
		font-variant-numeric: tabular-nums;
		width: 1.6em;
		text-align: right;
	}

	.primary {
		font-family: var(--font-display);
		font-size: var(--font-size-md);
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		min-width: 0;
	}

	.meta {
		display: inline-flex;
		align-items: baseline;
		gap: var(--space-2);
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.meta .artist {
		color: var(--text-tertiary);
		max-width: 16ch;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.meta .dot {
		color: var(--text-tertiary);
	}

	.meta .value {
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
	}

	.value {
		font-family: var(--font-mono);
		font-size: var(--font-size-sm);
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}

	@media (prefers-reduced-motion: reduce) {
		.hit {
			transition: none;
		}
	}
</style>
