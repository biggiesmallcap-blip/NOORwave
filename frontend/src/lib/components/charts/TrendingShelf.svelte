<!--
  Unified trending shelf. One mural, three scopes:
    [Worldwide] [Country] [Genre]
  When `country` or `genre` is selected, a secondary chip row appears with the
  curated list from /api/charts/lastfm/{countries,genres}.

  Every scope is a cached query (cachedApi.trendingQuery), so revisits and app
  restarts paint the last list instantly and revalidate in the background.

  Mounted by the Charts page.
-->
<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { get } from 'svelte/store';
	import {
		type ChartEntry,
		type TidalPlayable,
		type Track,
		type LastfmCountry,
		type LastfmGenre,
	} from '$lib/api/client';
	import { playTrackNow } from '$lib/stores/player';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { playChartTidalTrack } from '$lib/player/play_trending';
	import { canPlayTrack, getPlayableLabel } from '$lib/player/playable';
	import { buildTrackMenu, buildTidalTrackMenu } from '$lib/player/track_menu';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import {
		selectedTrendingMode,
		selectedCountry,
		selectedGenre,
		type TrendingMode,
	} from '$lib/stores/trending-prefs';
	import { cachedApi, type TrendingQuery } from '$lib/cache/api_queries';
	import ChartMural, { type ChartMuralItem } from '$lib/components/charts/ChartMural.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import ErrorState from '$lib/components/ui/ErrorState.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import FilterChip from '$lib/components/ui/FilterChip.svelte';
	import { composeTidalArtQuery, peekTidalArt } from '$lib/actions/lazy-tidal-art';
	import { usableArtwork } from '$lib/utils/artwork';

	interface Props {
		limit?: number;
	}
	let { limit = 12 }: Props = $props();

	// `tidal` is intentionally absent - the editorial-chart endpoint returns
	// 404 ("not confirmed" in the Tidal client warning), so exposing the tab
	// would always render an empty state. Add it back here when that endpoint
	// is sorted; the store/type/backend route still accept the value.
	const MODES: { value: TrendingMode; label: string }[] = [
		{ value: 'worldwide', label: 'Worldwide' },
		{ value: 'country', label: 'Country' },
		{ value: 'genre', label: 'Genre' },
	];
	const ROTATE_MS = 8000;

	let countries = $state<LastfmCountry[]>([]);
	let genres = $state<LastfmGenre[]>([]);
	let countriesReady = $state(false);
	let genresReady = $state(false);

	let tracks = $state<ChartEntry[]>([]);
	// Which scope `tracks` belongs to. While another scope loads, the old list
	// stays on screen dimmed instead of blanking to a skeleton.
	let tracksToken = $state('');
	let activeToken = $state('');
	// Default to loading=true so first render doesn't briefly paint the empty
	// state before the query reports in.
	let loading = $state(true);
	let refreshing = $state(false);
	let error = $state<unknown>(null);
	let currentEntryIndex = $state(0);
	let muralPaused = $state(false);
	let resolvingEntries = $state<Record<string, boolean>>({});
	let lazyArtwork = $state<Record<string, string>>({});
	let reloadTick = $state(0);
	let showingOtherScope = $derived(tracks.length > 0 && tracksToken !== activeToken);
	let visibleEntries = $derived(tracks.slice(0, limit));
	let currentEntry = $derived(visibleEntries[currentEntryIndex] ?? visibleEntries[0] ?? null);
	let muralItems = $derived<ChartMuralItem[]>(
		visibleEntries.map((entry, index) => ({
			id: entryKey(entry, index),
			title: entryTitle(entry),
			subtitle: entrySubtitle(entry, index),
			artwork: entryArtwork(entry, index),
			fallbackText: entryFallbackText(entry),
			tileLabel: `Select ${entryTitle(entry)}`,
			tileTitle: `${index + 1}. ${entryTitle(entry)} - ${entryArtist(entry) ?? 'Unknown artist'}`,
			lazy: {
				enabled: needsLazyArtwork(entry, index),
				query: { artist: entryArtist(entry), title: entryTitle(entry) },
				onResolve: (url) => {
					lazyArtwork = { ...lazyArtwork, [entryKey(entry, index)]: url };
				},
			},
		})),
	);

	function tokenFor(mode: TrendingMode, country: string, genre: string): string {
		if (mode === 'country') return `country:${country}`;
		if (mode === 'genre') return `genre:${genre}`;
		return mode;
	}

	// Per-entry key for the grid each-block. Last.fm-only entries arrive with
	// `tidal_playable.tidal_id === 0` (placeholder), so `??` falls through to
	// the index-based fallback, since 0 is falsy-but-not-nullish - without
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

	function optionalStringField(entry: ChartEntry, key: 'display_title' | 'display_subtitle'): string | null {
		const value = (entry as unknown as Record<string, unknown>)[key];
		return typeof value === 'string' && value.trim() ? value : null;
	}

	function entryTitle(entry: ChartEntry): string {
		return optionalStringField(entry, 'display_title') ?? entry.local_track?.title ?? entry.tidal_playable?.title ?? 'Unknown track';
	}

	function entryArtist(entry: ChartEntry): string | null {
		return optionalStringField(entry, 'display_subtitle') ?? entry.local_track?.artist_name ?? entry.tidal_playable?.artist_name ?? null;
	}

	function entryTarget(entry: ChartEntry): Track | TidalPlayable | null {
		return entry.local_track ?? entry.tidal_playable ?? null;
	}

	function entryArtwork(entry: ChartEntry, index: number): string | null {
		return usableArtwork(
			lazyArtwork[entryKey(entry, index)],
			entry.local_track?.artwork_url,
			entry.tidal_playable?.artwork_url,
			entry.image_url,
			// Previously-resolved artwork from the persistent cache, so the shelf
			// paints a full collage on first launch instead of empty tiles and
			// swaps to fresh art as live lookups land.
			peekTidalArt(composeTidalArtQuery(entryArtist(entry), entryTitle(entry))),
		);
	}

	function needsLazyArtwork(entry: ChartEntry, index: number): boolean {
		return entryArtwork(entry, index) === null;
	}

	function entryFallbackText(entry: ChartEntry): string {
		return (entryTitle(entry).trim()[0] ?? 'N').toUpperCase();
	}

	function entrySubtitle(entry: ChartEntry, index: number): string {
		return `#${index + 1} - ${entryArtist(entry) ?? 'Unknown artist'}`;
	}

	function isEntryUnresolved(entry: ChartEntry): boolean {
		return entry.local_track === null &&
			entry.tidal_playable !== null &&
			entry.tidal_playable.tidal_id <= 0;
	}

	function isEntryPlayable(entry: ChartEntry): boolean {
		const target = entryTarget(entry);
		return target !== null && (canPlayTrack(target) || isEntryUnresolved(entry));
	}

	function entryStatusLabel(entry: ChartEntry, index: number): string {
		const key = entryKey(entry, index);
		if (resolvingEntries[key]) return 'Resolving';
		if (entry.local_track) return 'In library';
		if (isEntryUnresolved(entry)) return 'Matched on TIDAL when played';
		if (entry.tidal_playable) return 'TIDAL ready';
		return 'Unavailable';
	}

	function entryActionLabel(entry: ChartEntry, index: number): string {
		const key = entryKey(entry, index);
		if (resolvingEntries[key]) return 'Resolving...';
		// Pressing it finds the song on TIDAL and plays it, so it says Play.
		if (isEntryUnresolved(entry)) return 'Play';
		const target = entryTarget(entry);
		return target ? getPlayableLabel(target) : 'Unavailable';
	}

	onMount(() => {
		// Migrate stale 'tidal' from the pre-merge source key before reads happen.
		if (!MODES.some((m) => m.value === get(selectedTrendingMode))) {
			selectedTrendingMode.set('worldwide');
		}
		// The curated lists are server constants: cached for days, and a failed
		// fetch still unblocks the shelf (the chip row just stays empty).
		const unsubCountries = cachedApi.lastfmCountriesQuery().subscribe((state) => {
			if (state.data) countries = state.data.countries;
			if (state.data || state.error) countriesReady = true;
		});
		const unsubGenres = cachedApi.lastfmGenresQuery().subscribe((state) => {
			if (state.data) genres = state.data.genres;
			if (state.data || state.error) genresReady = true;
		});
		return () => {
			unsubCountries();
			unsubGenres();
		};
	});

	$effect(() => {
		if (currentEntryIndex >= visibleEntries.length) currentEntryIndex = 0;
	});

	$effect(() => {
		if (visibleEntries.length <= 1) return;
		const timer = setInterval(() => {
			if (!muralPaused) jumpEntry(1);
		}, ROTATE_MS);
		return () => clearInterval(timer);
	});

	function trendingQuery(mode: TrendingMode, country: string, genre: string): TrendingQuery {
		if (mode === 'tidal') return { source: 'tidal', limit };
		if (mode === 'country') return { source: 'lastfm', limit, country };
		if (mode === 'genre') return { source: 'lastfm', limit, tag: genre };
		return { source: 'lastfm', limit };
	}

	// One cached query per scope (api_queries chartOptions): a revisited scope
	// paints its last list instantly - across navigations and app restarts -
	// and revalidates in the background. Re-runs on any scope store change;
	// the subscription is torn down with the effect, so a late answer for an
	// old scope can never land on the new one.
	$effect(() => {
		const mode = $selectedTrendingMode;
		const country = $selectedCountry;
		const genre = $selectedGenre;
		void reloadTick;
		if (mode === 'country' && !countriesReady) return;
		if (mode === 'genre' && !genresReady) return;

		const token = tokenFor(mode, country, genre);
		activeToken = token;
		const query = cachedApi.trendingQuery(trendingQuery(mode, country, genre));
		return query.subscribe((state) => untrack(() => {
			if (state.data) {
				if (tracksToken !== token) currentEntryIndex = 0;
				tracks = state.data.tracks ?? [];
				tracksToken = token;
			}
			loading = state.loading;
			refreshing = state.refreshing;
			error = state.data ? null : state.error;
			if (state.error && !state.data) console.error('[trending] fetch failed', { token, error: state.error });
		}));
	});

	function retry() {
		reloadTick += 1;
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

	function selectEntry(index: number) {
		currentEntryIndex = index;
	}

	function jumpEntry(delta: number) {
		if (visibleEntries.length === 0) return;
		currentEntryIndex = (currentEntryIndex + delta + visibleEntries.length) % visibleEntries.length;
	}

	async function playEntry(entry: ChartEntry, index: number) {
		const target = entryTarget(entry);
		if (!target || (!canPlayTrack(target) && !isEntryUnresolved(entry))) return;
		if (entry.local_track) {
			onTrack(entry.local_track);
			return;
		}
		if (!entry.tidal_playable) return;
		const key = entryKey(entry, index);
		resolvingEntries = { ...resolvingEntries, [key]: isEntryUnresolved(entry) };
		try {
			await playChartTidalTrack(entry.tidal_playable);
		} finally {
			resolvingEntries = { ...resolvingEntries, [key]: false };
		}
	}

	function handleEntryContext(e: MouseEvent, entry: ChartEntry) {
		e.preventDefault();
		e.stopPropagation();
		const local = entry.local_track;
		if (local) {
			openContextMenu(e, buildTrackMenu(local), local.title);
			return;
		}
		const tidal = entry.tidal_playable;
		if (tidal) {
			openContextMenu(e, buildTidalTrackMenu(tidal), tidal.title);
		}
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
	<SectionHeader title="Trending" subtitle={$selectedTrendingMode === 'tidal' ? subLabel : `${subLabel} on Last.fm`} variant="charts" level={2}>
		{#snippet actions()}
			<Segmented
				options={MODES}
				value={$selectedTrendingMode}
				label="Trending scope"
				onchange={pickMode}
			/>
		{/snippet}
	</SectionHeader>

	<!-- Always-rendered subrow; content swaps by mode. Reserves stable vertical
	     space so the mural below doesn't jump when modes change. -->
	<div class="chip-row" role="group" aria-label={$selectedTrendingMode === 'genre' ? 'Genre' : 'Country'}>
		{#if $selectedTrendingMode === 'country'}
			{#each countries as c (c.code)}
				<FilterChip pressed={c.code === $selectedCountry} onclick={() => pickCountry(c.code)}>
					{c.label}
				</FilterChip>
			{/each}
		{:else if $selectedTrendingMode === 'genre'}
			{#each genres as g (g.key)}
				<FilterChip pressed={g.key === $selectedGenre} onclick={() => pickGenre(g.key)}>
					{g.label}
				</FilterChip>
			{/each}
		{/if}
	</div>

	{#if error && (tracks.length === 0 || showingOtherScope)}
		<ErrorState title="Couldn't load this chart" {error} onretry={retry} />
	{:else if tracks.length > 0 || loading}
		<ChartMural
			items={muralItems}
			currentIndex={currentEntryIndex}
			ariaLabel={`Last.fm ${subLabel} top ${visibleEntries.length}`}
			title={currentEntry ? entryTitle(currentEntry) : ''}
			subtitle={currentEntry ? entrySubtitle(currentEntry, currentEntryIndex) : ''}
			metric={currentEntry ? currentEntry.genre ?? entryStatusLabel(currentEntry, currentEntryIndex) : ''}
			actionLabel={currentEntry ? entryActionLabel(currentEntry, currentEntryIndex) : 'Unavailable'}
			actionDisabled={!currentEntry || !isEntryPlayable(currentEntry)}
			accent="lastfm"
			loading={tracks.length === 0}
			loadingLabel="Loading Last.fm chart"
			refreshing={refreshing}
			stale={showingOtherScope && loading}
			onSelect={selectEntry}
			onJump={jumpEntry}
			onPlay={() => currentEntry && playEntry(currentEntry, currentEntryIndex)}
			onCardContext={(event) => currentEntry && handleEntryContext(event, currentEntry)}
			onItemContext={(event, index) => {
				const entry = visibleEntries[index];
				if (entry) handleEntryContext(event, entry);
			}}
			onPauseChange={(paused) => muralPaused = paused}
		/>
	{:else}
		<EmptyState title="Nothing trending here yet" copy="Try another country, genre, or scope." />
	{/if}
</section>

<style>
	.trending-shelf {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.chip-row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-1);
		/* Reserves one row of chip height even when worldwide mode renders no
		   chips, so switching modes doesn't shift the mural below. */
		min-height: var(--control-h);
	}
</style>
