<script lang="ts">
	// Find tracks to add without leaving the playlist: the library first, then
	// TIDAL. A TIDAL result with no local row is imported (as radio does) first,
	// because playlist rows always point at local tracks.
	import { onDestroy } from 'svelte';
	import { api, ApiError, type TidalSearchTrack, type Track } from '$lib/api/client';
	import { invalidatePlaylistCaches } from '$lib/cache/ws_events';
	import { showToast } from '$lib/stores/toast';
	import { tidalSearchTrackToPlayable } from '$lib/utils/track';
	import { formatTrackDuration } from '$lib/utils/format';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';

	let { playlistId, present, onAdded, onClose }: {
		playlistId: number;
		/** Local track ids already in the playlist. */
		present: ReadonlySet<number>;
		onAdded: () => void;
		onClose: () => void;
	} = $props();

	type Result = {
		key: string;
		title: string;
		subtitle: string;
		artwork: string | null;
		durationMs: number | null;
		localId: number | null;
		resolve: () => Promise<number>;
	};

	const DEBOUNCE_MS = 250;
	const LIMIT = 8;

	let query = $state('');
	let results = $state<Result[]>([]);
	let searched = $state(false);
	let searching = $state(false);
	let unavailable = $state(false);
	let busyKeys = $state(new Set<string>());
	let addedKeys = $state(new Set<string>());
	let debounce: ReturnType<typeof setTimeout> | undefined;
	let controller: AbortController | null = null;

	onDestroy(() => {
		clearTimeout(debounce);
		controller?.abort();
	});

	function schedule() {
		clearTimeout(debounce);
		const q = query.trim();
		if (!q) {
			controller?.abort();
			results = [];
			searched = false;
			return;
		}
		debounce = setTimeout(() => void search(q), DEBOUNCE_MS);
	}

	function fromLibrary(track: Track): Result {
		return {
			key: `l:${track.id}`,
			title: track.title,
			subtitle: [track.artist_name, track.album_title].filter(Boolean).join(' - '),
			artwork: track.artwork_url ?? null,
			durationMs: track.duration_ms,
			localId: track.id,
			resolve: async () => track.id,
		};
	}

	function fromTidal(track: TidalSearchTrack): Result {
		return {
			key: `t:${track.tidal_id}`,
			title: track.title,
			subtitle: [track.artist_name, track.album_title, 'TIDAL'].filter(Boolean).join(' - '),
			artwork: track.artwork_url,
			durationMs: track.duration_ms,
			localId: track.local_id ?? null,
			resolve: async () =>
				track.local_id ?? (await api.importTidalTrackForRadio(tidalSearchTrackToPlayable(track))).local_id,
		};
	}

	async function search(q: string) {
		controller?.abort();
		const own = new AbortController();
		controller = own;
		searching = true;
		const [library, tidal] = await Promise.allSettled([
			api.search(q, LIMIT, own.signal),
			api.searchTidal(q, LIMIT, own.signal),
		]);
		if (own.signal.aborted) return;
		const local = library.status === 'fulfilled' ? library.value.tracks.map(fromLibrary) : [];
		const seen = new Set(local.map((result) => result.localId));
		const remote =
			tidal.status === 'fulfilled'
				? tidal.value.tracks
						.map(fromTidal)
						.filter((result) => result.localId === null || !seen.has(result.localId))
				: [];
		results = [...local, ...remote];
		unavailable = library.status === 'rejected' && tidal.status === 'rejected';
		searched = true;
		searching = false;
	}

	async function add(result: Result) {
		if (busyKeys.has(result.key)) return;
		busyKeys = new Set(busyKeys).add(result.key);
		try {
			const id = await result.resolve();
			const { added } = await api.addTracksToPlaylist(playlistId, [id]);
			invalidatePlaylistCaches();
			addedKeys = new Set(addedKeys).add(result.key);
			if (!added) showToast(`"${result.title}" is already in this playlist`, 'info');
			onAdded();
		} catch (reason) {
			// The API's 4xx messages are written for people ("This track is not
			// available on TIDAL"); anything else is not.
			const message =
				reason instanceof ApiError && reason.status !== 500 && !/^API error/i.test(reason.message)
					? reason.message
					: `Couldn't add "${result.title}". Try again.`;
			showToast(message, 'error');
		} finally {
			const next = new Set(busyKeys);
			next.delete(result.key);
			busyKeys = next;
		}
	}

	function isIn(result: Result) {
		return addedKeys.has(result.key) || (result.localId !== null && present.has(result.localId));
	}
</script>

<section class="add-tracks" aria-label="Add tracks">
	<div class="search-row">
		<SearchField
			bind:value={query}
			size="sm"
			fill
			autofocus
			placeholder="Search your library and TIDAL"
			ariaLabel="Search tracks to add"
			oninput={schedule}
		/>
		<button type="button" class="btn btn-ghost" onclick={onClose}>Done</button>
	</div>

	{#if unavailable}
		<p class="note t-body" role="alert">Search isn't available right now. Try again in a moment.</p>
	{:else if searched && !searching && results.length === 0}
		<p class="note t-body">Nothing matches "{query.trim()}".</p>
	{:else if results.length}
		<ol class="results" class:refreshing={searching} aria-busy={searching}>
			{#each results as result (result.key)}
				{@const inPlaylist = isIn(result)}
				<li class="result">
					<ArtworkImage src={result.artwork} alt="" size={80} className="result-art" fallbackText={result.title.slice(0, 2)} decorative />
					<div class="result-copy">
						<span class="t-row-title" title={result.title}>{result.title}</span>
						<span class="t-meta" title={result.subtitle}>{result.subtitle}</span>
					</div>
					<span class="duration t-micro">{formatTrackDuration(result.durationMs)}</span>
					<button
						type="button"
						class="btn btn-icon"
						disabled={inPlaylist || busyKeys.has(result.key)}
						aria-label={inPlaylist ? `${result.title} is in this playlist` : `Add ${result.title}`}
						title={inPlaylist ? 'In this playlist' : 'Add to playlist'}
						onclick={() => void add(result)}
					>
						{#if inPlaylist}
							<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 12l5 5 9-10" /></svg>
						{:else}
							<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>
						{/if}
					</button>
				</li>
			{/each}
		</ol>
	{/if}
</section>

<style>
	.add-tracks {
		display: grid;
		gap: var(--group-gap);
		max-width: var(--measure-form);
		margin-bottom: var(--space-5);
	}

	.search-row {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	.note {
		margin: 0;
		color: var(--text-secondary);
	}

	.results {
		display: grid;
		gap: var(--row-gap);
		margin: 0;
		padding: 0;
		list-style: none;
		transition: opacity var(--motion-fast);
	}

	.results.refreshing {
		opacity: 0.55;
	}

	.result {
		display: grid;
		grid-template-columns: 40px minmax(0, 1fr) auto auto;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-1) var(--space-2);
		border-radius: var(--radius-sm);
		transition: background var(--motion-fast);
	}

	.result:hover {
		background: var(--bg-hover);
	}

	.result :global(.result-art) {
		width: 40px;
		height: 40px;
		border-radius: var(--radius-sm);
		object-fit: cover;
	}

	.result-copy {
		display: grid;
		min-width: 0;
	}

	.duration {
		font-variant-numeric: tabular-nums;
	}
</style>
