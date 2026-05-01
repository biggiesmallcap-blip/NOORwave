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

	onMount(() => {
		// If a stale 'tidal' mode is in localStorage from the legacy migration,
		// nudge to a working mode so the user doesn't open to an empty shelf.
		if (!MODES.some((m) => m.id === $selectedTrendingMode)) {
			selectedTrendingMode.set('worldwide');
		}
		void loadCurated();
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

	// Refetch whenever the effective scope changes. The reads of all three
	// stores below establish reactive deps; we then build a dedup token and
	// skip if it hasn't changed (avoids double-fires on no-op store writes).
	let lastToken = '';
	$effect(() => {
		const mode = $selectedTrendingMode;
		const country = $selectedCountry;
		const genre = $selectedGenre;

		// Don't fetch country/genre modes until curated lists are loaded —
		// we'd otherwise send the default code/key and immediately refetch.
		if ((mode === 'country' || mode === 'genre') && !curatedLoaded) return;

		const token =
			mode === 'country'
				? `country:${country}`
				: mode === 'genre'
					? `genre:${genre}`
					: mode;
		if (token === lastToken) return;
		lastToken = token;
		void load(mode, country, genre);
	});

	async function load(mode: TrendingMode, country: string, genre: string) {
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
			tracks = data.tracks ?? [];
		} catch (e) {
			console.error('Failed to load trending:', e);
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
			<p class="eyebrow">Now moving</p>
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
			{#each tracks.slice(0, limit) as entry, i (entry.local_track?.id ?? entry.tidal_playable?.tidal_id ?? `idx-${i}`)}
				<TrendingCard {entry} index={i} {onTrack} onTidal={playChartTidalTrack} />
			{/each}
		</div>
	{:else if loading}
		<!-- header indicator handles this; avoid layout shift -->
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
		color: var(--text-muted, #888);
		font-weight: 500;
		font-size: 0.95rem;
		margin-left: 2px;
	}

	.eyebrow {
		font-size: 0.7rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-muted, #888);
		margin: 0;
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

	.chip:hover { color: var(--text, #fff); }

	.chip.active {
		background: rgba(255, 255, 255, 0.12);
		color: var(--text, #fff);
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
</style>
