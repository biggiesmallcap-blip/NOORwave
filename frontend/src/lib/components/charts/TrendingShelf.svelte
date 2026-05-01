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

	const MODES: { id: TrendingMode; label: string }[] = [
		{ id: 'worldwide', label: 'Worldwide' },
		{ id: 'country', label: 'Country' },
		{ id: 'genre', label: 'Genre' },
		{ id: 'tidal', label: 'Tidal' },
	];

	let countries = $state<LastfmCountry[]>([]);
	let genres = $state<LastfmGenre[]>([]);
	let curatedLoaded = $state(false);

	let tracks = $state<ChartEntry[]>([]);
	let loading = $state(false);
	let error = $state(false);

	onMount(() => {
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
			{#if loading}
				<span class="loading-indicator">Loading…</span>
			{/if}
		</div>
	</div>

	{#if $selectedTrendingMode === 'country' && countries.length > 0}
		<div class="chip-row" role="tablist" aria-label="Country">
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
		</div>
	{:else if $selectedTrendingMode === 'genre' && genres.length > 0}
		<div class="chip-row" role="tablist" aria-label="Genre">
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
		</div>
	{/if}

	{#if tracks.length > 0}
		<div class="trending-grid">
			{#each tracks.slice(0, limit) as entry, i (`${$selectedTrendingMode}-${$selectedCountry}-${$selectedGenre}-${i}-${entry.local_track?.id ?? entry.tidal_playable?.tidal_id ?? i}`)}
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
		flex-wrap: wrap;
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
