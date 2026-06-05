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
	import { onDestroy, onMount } from 'svelte';
	import { get } from 'svelte/store';
	import {
		api,
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
	import { getCached, putCached } from '$lib/stores/trending-cache';
	import ChartMural, { type ChartMuralItem } from '$lib/components/charts/ChartMural.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';

	interface Props {
		limit?: number;
	}
	let { limit = 12 }: Props = $props();

	// `tidal` is intentionally absent - the editorial-chart endpoint returns
	// 404 ("not confirmed" in the Tidal client warning), so exposing the tab
	// would always render an empty state. Add it back here when that endpoint
	// is sorted; the store/type/backend route still accept the value.
	const MODES: { id: TrendingMode; label: string }[] = [
		{ id: 'worldwide', label: 'Worldwide' },
		{ id: 'country', label: 'Country' },
		{ id: 'genre', label: 'Genre' },
	];
	const ROTATE_MS = 8000;

	let countries = $state<LastfmCountry[]>([]);
	let genres = $state<LastfmGenre[]>([]);
	let curatedLoaded = $state(false);

	let tracks = $state<ChartEntry[]>([]);
	// Default to loading=true so first render doesn't briefly paint the empty
	// state before the on-mount fetch flips it on.
	let loading = $state(true);
	let error = $state(false);
	let currentEntryIndex = $state(0);
	let muralPaused = $state(false);
	let resolvingEntries = $state<Record<string, boolean>>({});
	let lazyArtwork = $state<Record<string, string>>({});

	let lastToken = '';
	let chartLoadSeq = 0;
	let curatedLoadSeq = 0;
	let destroyed = false;
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

	const LASTFM_PLACEHOLDER_HASH = '2a96cbd8b46e442fc41c2b86b821562f';
	function usableArtwork(...candidates: (string | null | undefined)[]): string | null {
		for (const candidate of candidates) {
			if (!candidate) continue;
			const trimmed = candidate.trim();
			if (!trimmed) continue;
			if (trimmed.includes(LASTFM_PLACEHOLDER_HASH)) continue;
			return trimmed;
		}
		return null;
	}

	function entryArtwork(entry: ChartEntry, index: number): string | null {
		return usableArtwork(
			lazyArtwork[entryKey(entry, index)],
			entry.local_track?.artwork_url,
			entry.tidal_playable?.artwork_url,
			entry.image_url,
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
		if (isEntryUnresolved(entry)) return 'Resolve on TIDAL';
		if (entry.tidal_playable) return 'TIDAL ready';
		return 'Unavailable';
	}

	function entryActionLabel(entry: ChartEntry, index: number): string {
		const key = entryKey(entry, index);
		if (resolvingEntries[key]) return 'Resolving...';
		if (isEntryUnresolved(entry)) return 'Resolve on TIDAL';
		const target = entryTarget(entry);
		return target ? getPlayableLabel(target) : 'Unavailable';
	}

	function currentKindLabel(): string {
		return `Last.fm top ${visibleEntries.length} - ${subLabel}`;
	}

	onMount(() => {
		// Migrate stale 'tidal' from the pre-merge source key before reads happen.
		if (!MODES.some((m) => m.id === get(selectedTrendingMode))) {
			selectedTrendingMode.set('worldwide');
		}
		void loadCurated();
	});

	onDestroy(() => {
		destroyed = true;
		chartLoadSeq += 1;
		curatedLoadSeq += 1;
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
			chartLoadSeq += 1;
			tracks = cached;
			currentEntryIndex = 0;
			loading = false;
			error = false;
			return;
		}
		void load(mode, country, genre, token);
	});

	async function loadCurated() {
		const seq = ++curatedLoadSeq;
		try {
			const [c, g] = await Promise.all([
				api.getLastfmCountries(),
				api.getLastfmGenres(),
			]);
			if (destroyed || seq !== curatedLoadSeq) return;
			countries = c.countries;
			genres = g.genres;
		} catch (e) {
			if (destroyed || seq !== curatedLoadSeq) return;
			console.error('Failed to load curated chart lists:', e);
		} finally {
			if (!destroyed && seq === curatedLoadSeq) curatedLoaded = true;
		}
	}

	async function load(mode: TrendingMode, country: string, genre: string, token: string) {
		const seq = ++chartLoadSeq;
		// Keep `tracks` populated while we fetch - replacing only when new data
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
			if (!isCurrentChartLoad(seq, token)) return;
			const next = data.tracks ?? [];
			tracks = next;
			currentEntryIndex = 0;
			// Only cache non-empty payloads so a transient 5xx returning [] doesn't
			// poison the cache for 6h.
			if (next.length > 0) putCached(token, next);
		} catch (e) {
			if (!isCurrentChartLoad(seq, token)) return;
			console.error('[trending] fetch failed', { token, error: e });
			tracks = [];
			error = true;
		} finally {
			if (isCurrentChartLoad(seq, token)) loading = false;
		}
	}

	function isCurrentChartLoad(seq: number, token: string): boolean {
		return !destroyed && seq === chartLoadSeq && token === lastToken;
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
	<SectionHeader eyebrow="From Last.fm - Now moving" title="Trending" subtitle={subLabel} variant="charts" level={2}>
		{#snippet actions()}
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
				<!-- Always-rendered, fixed-width slot - flips opacity instead of mounting/unmounting,
				     so the chip group doesn't reflow when fetches start/finish. -->
				<span class="loading-indicator" class:visible={loading} aria-hidden={!loading}>Loading...</span>
			</div>
		{/snippet}
	</SectionHeader>

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

	{#if tracks.length > 0 || loading}
		<ChartMural
			items={muralItems}
			currentIndex={currentEntryIndex}
			ariaLabel={`Last.fm ${subLabel} top ${visibleEntries.length}`}
			kindLabel={currentKindLabel()}
			title={currentEntry ? entryTitle(currentEntry) : ''}
			subtitle={currentEntry ? entrySubtitle(currentEntry, currentEntryIndex) : ''}
			metric={currentEntry ? currentEntry.genre ?? entryStatusLabel(currentEntry, currentEntryIndex) : ''}
			actionLabel={currentEntry ? entryActionLabel(currentEntry, currentEntryIndex) : 'Unavailable'}
			actionDisabled={!currentEntry || !isEntryPlayable(currentEntry)}
			accent="lastfm"
			loading={loading && tracks.length === 0}
			loadingLabel="Loading Last.fm chart"
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
	{:else if error}
		<EmptyState title="Couldn't load this chart" copy="Try another scope or check the Last.fm key in Settings." />
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

	.trending-controls {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex-wrap: nowrap; /* keeps chip-group + loading slot on a single line */
		min-width: 0;
	}

	.chip-group {
		display: inline-flex;
		gap: var(--space-1);
		padding: var(--space-1);
		background: var(--panel-bg);
		border: 1px solid var(--panel-border);
		border-radius: 999px;
	}

	.chip {
		background: var(--panel-bg);
		border: 1px solid var(--panel-border);
		color: var(--text-muted);
		font: inherit;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		padding: var(--space-2) var(--space-3);
		border-radius: 999px;
		cursor: pointer;
		white-space: nowrap;
		transition: background var(--motion-base), border-color var(--motion-base), color var(--motion-base);
	}

	.chip:hover,
	.chip:focus-visible {
		background: var(--bg-hover);
		border-color: var(--accent-line);
		color: var(--text-primary);
		outline: none;
	}

	.chip.active {
		background: var(--bg-hover);
		border-color: var(--accent-line);
		color: var(--text-primary);
	}

	.chip-row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-1);
		/* Reserves one row of chip height even when worldwide mode renders no
		   chips, so switching modes doesn't shift the grid below. */
		min-height: clamp(28px, 2vw, 36px);
	}

	.chip.secondary {
		background: var(--panel-bg);
	}
	.chip.secondary.active {
		background: var(--bg-hover);
	}

	.loading-indicator {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
		font-style: italic;
		/* Reserve the slot so the chip group never reflows when loading toggles. */
		min-width: clamp(52px, 4vw, 68px);
		opacity: 0;
		transition: opacity var(--motion-fast);
		pointer-events: none;
	}
	.loading-indicator.visible {
		opacity: 1;
	}

</style>
