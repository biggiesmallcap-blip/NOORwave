<!--
  Unified trending shelf. One grid, four scopes:
    [Worldwide] [Country] [Genre] [Tidal]
  When `country` or `genre` is selected, a secondary chip row appears with the
  curated list from /api/charts/lastfm/{countries,genres}.

  Replaces the previous separate "Trending" + "Trending by Country" +
  "Trending by Genre" shelves so we don't render three copies of the same grid.

  Mounted by Home (always) and Search (only when the search query is empty).
-->
<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import {
		api,
		type ChartEntry,
		type Track,
		type LastfmCountry,
		type LastfmGenre,
	} from '$lib/api/client';
	import { playTrackNow } from '$lib/stores/player';
	import { playChartTidalTrack } from '$lib/player/play_trending';
	import {
		selectedTrendingMode,
		selectedCountry,
		selectedGenre,
		type TrendingMode,
	} from '$lib/stores/trending-prefs';
	import { getCached, putCached } from '$lib/stores/trending-cache';
	import TrendingCard from '$lib/components/TrendingCard.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';

	interface Props {
		limit?: number;
	}
	let { limit = 12 }: Props = $props();

	// `tidal` is intentionally absent — the editorial-chart endpoint returns
	// 404 ("not confirmed" in the Tidal client warning), so exposing the tab
	// would always render an empty state. Add it back here when that endpoint
	// is sorted; the store/type/backend route still accept the value.
	const MODES: { id: TrendingMode; label: string }[] = [
		{ id: 'worldwide', label: 'Worldwide' },
		{ id: 'country', label: 'Country' },
		{ id: 'genre', label: 'Genre' },
	];

	let countries = $state<LastfmCountry[]>([]);
	let genres = $state<LastfmGenre[]>([]);
	let curatedLoaded = $state(false);

	let tracks = $state<ChartEntry[]>([]);
	// Default to loading=true so first render doesn't briefly paint the empty
	// state before the on-mount fetch flips it on.
	let loading = $state(true);
	let error = $state(false);

	let lastToken = '';

	function tokenFor(mode: TrendingMode, country: string, genre: string): string {
		if (mode === 'country') return `country:${country}`;
		if (mode === 'genre') return `genre:${genre}`;
		return mode;
	}

	// Per-entry key for the grid each-block. Last.fm-only entries arrive with
	// `tidal_playable.tidal_id === 0` (placeholder), so `??` falls through to
	// the index-based fallback, since 0 is falsy-but-not-nullish — without
	// this, every unresolved card collides on key `0` and Svelte throws
	// `each_key_duplicate`, which prevents the whole shelf from rendering.
	function entryKey(entry: ChartEntry, i: number): string {
		const localId = entry.local_track?.id;
		if (typeof localId === 'number' && localId > 0) return `local:${localId}`;
		const tidalId = entry.tidal_playable?.tidal_id;
		if (typeof tidalId === 'number' && tidalId > 0) return `tidal:${tidalId}`;
		const artist = entry.tidal_playable?.artist_name ?? entry.local_track?.artist_name ?? '';
		const title = entry.tidal_playable?.title ?? entry.local_track?.title ?? '';
		return `lf:${i}:${artist}:${title}`;
	}

	onMount(() => {
		// Migrate stale 'tidal' from the pre-merge source key before reads happen.
		if (!MODES.some((m) => m.id === get(selectedTrendingMode))) {
			selectedTrendingMode.set('worldwide');
		}
		void loadCurated();
	});

	// Idiomatic Svelte 5: $effect tracks $store reads and re-runs on any change.
	// Fires once on mount AND on every subsequent store update. The lastToken
	// dedup makes no-op writes free; the curated guard delays country/genre
	// modes until the static lists land.
	$effect(() => {
		const mode = $selectedTrendingMode;
		const country = $selectedCountry;
		const genre = $selectedGenre;
		const ready = curatedLoaded;

		if ((mode === 'country' || mode === 'genre') && !ready) return;

		const token = tokenFor(mode, country, genre);
		if (token === lastToken) return;
		lastToken = token;

		// Cache hit: skip the network round-trip. 6h shared with the backend
		// keeps the shelf static across page navigations within the window.
		const cached = getCached(token);
		if (cached) {
			tracks = cached;
			loading = false;
			error = false;
			return;
		}
		void load(mode, country, genre, token);
	});

	async function loadCurated() {
		try {
			const [c, g] = await Promise.all([
				api.getLastfmCountries(),
				api.getLastfmGenres(),
			]);
			countries = c.countries;
			genres = g.genres;
		} catch (e) {
			console.error('Failed to load curated chart lists:', e);
		} finally {
			curatedLoaded = true;
		}
	}

	async function load(mode: TrendingMode, country: string, genre: string, token: string) {
		// Keep `tracks` populated while we fetch — replacing only when new data
		// lands avoids the flash-to-empty-state and the resulting grid reflow.
		loading = true;
		error = false;
		try {
			let data: { tracks: ChartEntry[] | null };
			if (mode === 'tidal') {
				data = await api.getTrending({ source: 'tidal', limit });
			} else if (mode === 'country') {
				data = await api.getTrending({ source: 'lastfm', limit, country });
			} else if (mode === 'genre') {
				data = await api.getTrending({ source: 'lastfm', limit, tag: genre });
			} else {
				data = await api.getTrending({ source: 'lastfm', limit });
			}
			const next = data.tracks ?? [];
			tracks = next;
			// Only cache non-empty payloads so a transient 5xx returning [] doesn't
			// poison the cache for 6h.
			if (next.length > 0) putCached(token, next);
		} catch (e) {
			console.error('[trending] fetch failed', { token, error: e });
			tracks = [];
			error = true;
		} finally {
			loading = false;
		}
	}

	function pickMode(m: TrendingMode) {
		if (m === $selectedTrendingMode) return;
		selectedTrendingMode.set(m);
	}
	function pickCountry(code: string) {
		if (code === $selectedCountry) return;
		selectedCountry.set(code);
	}
	function pickGenre(key: string) {
		if (key === $selectedGenre) return;
		selectedGenre.set(key);
	}

	function onTrack(t: Track) {
		void playTrackNow(t.id);
	}

	const subLabel = $derived.by(() => {
		const m = $selectedTrendingMode;
		if (m === 'country') {
			return countries.find((c) => c.code === $selectedCountry)?.label ?? $selectedCountry;
		}
		if (m === 'genre') {
			return genres.find((g) => g.key === $selectedGenre)?.label ?? $selectedGenre;
		}
		if (m === 'tidal') return 'Tidal editorial';
		return 'Worldwide';
	});
</script>

<section class="trending-shelf">
	<div class="section-header">
		<div class="section-title-group">
			<p class="eyebrow">From Last.fm <span class="eyebrow-dot" aria-hidden="true">·</span> Now moving</p>
			<h2>Trending <span class="sub">· {subLabel}</span></h2>
		</div>
		<div class="trending-controls">
			<div class="chip-group" role="tablist" aria-label="Trending scope">
				{#each MODES as m (m.id)}
					<button
						type="button"
						class="chip"
						class:active={m.id === $selectedTrendingMode}
						onclick={() => pickMode(m.id)}
						role="tab"
						aria-selected={m.id === $selectedTrendingMode}
					>
						{m.label}
					</button>
				{/each}
			</div>
			<!-- Always-rendered, fixed-width slot — flips opacity instead of mounting/unmounting,
			     so the chip group doesn't reflow when fetches start/finish. -->
			<span class="loading-indicator" class:visible={loading} aria-hidden={!loading}>Loading…</span>
		</div>
	</div>

	<!-- Always-rendered subrow; content swaps by mode. Reserves stable vertical
	     space so the grid below doesn't jump when modes change. -->
	<div class="chip-row" role="tablist" aria-label={$selectedTrendingMode === 'genre' ? 'Genre' : 'Country'}>
		{#if $selectedTrendingMode === 'country' && countries.length > 0}
			{#each countries as c (c.code)}
				<button
					type="button"
					class="chip secondary"
					class:active={c.code === $selectedCountry}
					onclick={() => pickCountry(c.code)}
					role="tab"
					aria-selected={c.code === $selectedCountry}
				>
					{c.label}
				</button>
			{/each}
		{:else if $selectedTrendingMode === 'genre' && genres.length > 0}
			{#each genres as g (g.key)}
				<button
					type="button"
					class="chip secondary"
					class:active={g.key === $selectedGenre}
					onclick={() => pickGenre(g.key)}
					role="tab"
					aria-selected={g.key === $selectedGenre}
				>
					{g.label}
				</button>
			{/each}
		{/if}
	</div>

	{#if tracks.length > 0}
		<div class="trending-grid">
			{#each tracks.slice(0, limit) as entry, i (entryKey(entry, i))}
				<TrendingCard {entry} index={i} {onTrack} onTidal={playChartTidalTrack} />
			{/each}
		</div>
	{:else if loading}
		<!-- Skeleton grid so the area isn't blank during the first fetch (was a
		     "did the page break?" UX cliff with the previous render-nothing branch). -->
		<div class="trending-grid">
			{#each Array.from({ length: Math.min(limit, 8) }) as _, i (i)}
				<div class="skeleton-card" aria-hidden="true"></div>
			{/each}
		</div>
	{:else if error}
		<EmptyState title="Couldn’t load this chart" copy="Try another scope or check the Last.fm key in Settings." />
	{:else}
		<EmptyState title="Nothing trending here yet" copy="Try another country, genre, or scope." />
	{/if}
</section>

<style>
	.trending-shelf {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}

	.section-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
		flex-wrap: wrap;
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

	.section-title-group h2 .sub {
		color: var(--text-muted);
		font-weight: 500;
		font-size: 0.95rem;
		margin-left: 2px;
	}

	.eyebrow {
		font-size: 0.7rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--service-lastfm);
		margin: 0;
	}
	.eyebrow-dot {
		color: var(--text-muted);
		margin: 0 4px;
	}

	.trending-controls {
		display: flex;
		align-items: center;
		gap: 12px;
		flex-wrap: nowrap; /* keeps chip-group + loading slot on a single line */
		min-width: 0;
	}

	.chip-group {
		display: inline-flex;
		gap: var(--space-1);
		padding: 2px;
		background: var(--panel-bg);
		border: 1px solid var(--panel-border);
		border-radius: 999px;
	}

	.chip {
		background: transparent;
		border: none;
		color: var(--text-muted);
		font: inherit;
		font-size: var(--font-size-xs);
		font-weight: 500;
		padding: 4px 10px;
		border-radius: 999px;
		cursor: pointer;
		transition: background var(--motion-fast), color var(--motion-fast);
	}

	.chip:hover { color: var(--text-primary); }

	.chip.active {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.chip-row {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		/* Reserves one row of chip height even when worldwide mode renders no
		   chips, so switching modes doesn't shift the grid below. */
		min-height: 28px;
	}

	.chip.secondary {
		background: rgba(255, 255, 255, 0.04);
	}
	.chip.secondary.active {
		background: rgba(255, 255, 255, 0.16);
	}

	.loading-indicator {
		font-size: 0.78rem;
		color: var(--text-muted);
		font-style: italic;
		/* Reserve the slot so the chip group never reflows when loading toggles. */
		min-width: 60px;
		opacity: 0;
		transition: opacity 0.18s ease;
		pointer-events: none;
	}
	.loading-indicator.visible {
		opacity: 1;
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

	.skeleton-card {
		aspect-ratio: 1;
		border-radius: var(--radius-md);
		background: linear-gradient(
			110deg,
			rgba(255, 255, 255, 0.04) 30%,
			rgba(255, 255, 255, 0.08) 50%,
			rgba(255, 255, 255, 0.04) 70%
		);
		background-size: 200% 100%;
		animation: skeleton-shimmer 1.4s ease-in-out infinite;
	}

	@keyframes skeleton-shimmer {
		0% { background-position: 200% 0; }
		100% { background-position: -200% 0; }
	}
</style>
