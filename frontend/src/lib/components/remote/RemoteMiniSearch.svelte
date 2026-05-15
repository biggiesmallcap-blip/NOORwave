<script lang="ts">
	import { api, type TidalSearchTrack, type Track } from '$lib/api/client';
	import {
		addTidalTrackToQueue,
		addTrackToQueue,
		playTidalTrackNext,
		playTidalTrackNow,
		playTrackNext,
		playTrackNow
	} from '$lib/stores/player';
	import { normalizeRemoteSearchQuery, shouldRunRemoteSearch } from '$lib/remote/search';

	let query = $state('');
	let open = $state(false);
	let busy = $state(false);
	let error = $state('');
	let mode = $state<'library' | 'tidal'>('library');
	let results = $state<Track[]>([]);
	let tidalResults = $state<TidalSearchTrack[]>([]);
	let searchSeq = 0;

	$effect(() => {
		const normalized = normalizeRemoteSearchQuery(query);
		const activeMode = mode;
		if (!shouldRunRemoteSearch(normalized)) {
			results = [];
			tidalResults = [];
			error = '';
			busy = false;
			return;
		}
		const seq = ++searchSeq;
		busy = true;
		const timer = setTimeout(() => {
			const request =
				activeMode === 'tidal'
					? api.searchTidal(normalized, 8).then((data) => {
							if (seq !== searchSeq) return;
							tidalResults = data.tracks;
							results = [];
							error = '';
						})
					: api.search(normalized, 8).then((data) => {
							if (seq !== searchSeq) return;
							results = data.tracks;
							tidalResults = [];
							error = '';
						});
			void request
				.catch(() => {
					if (seq !== searchSeq) return;
					results = [];
					tidalResults = [];
					error = 'Search failed.';
				})
				.finally(() => {
					if (seq === searchSeq) busy = false;
				});
		}, 180);
		return () => clearTimeout(timer);
	});
</script>

<section class="remote-search" aria-label="Mini search">
	<button type="button" class="remote-search-toggle" onclick={() => { open = !open; }}>
		Search
	</button>

	{#if open}
		<div class="remote-search-panel">
			<div class="remote-search-tabs" role="tablist" aria-label="Search source">
				<button type="button" class:active={mode === 'library'} onclick={() => { mode = 'library'; }}>Library</button>
				<button type="button" class:active={mode === 'tidal'} onclick={() => { mode = 'tidal'; }}>TIDAL</button>
			</div>

			<label>
				<span>Search {mode === 'tidal' ? 'TIDAL' : 'library'}</span>
				<input bind:value={query} type="search" inputmode="search" autocomplete="off" placeholder="Track, artist, album" />
			</label>

			{#if busy}
				<p class="remote-search-status">Searching...</p>
			{:else if error}
				<p class="remote-search-status">{error}</p>
			{:else if mode === 'library' && results.length > 0}
				<div class="remote-search-results">
					{#each results as track (track.id)}
						<article class="remote-search-row">
							<div>
								<strong>{track.title}</strong>
								<span>{track.artist_name ?? 'Unknown artist'}</span>
							</div>
							<div class="remote-search-actions">
								<button type="button" onclick={() => void playTrackNow(track.id)}>Play</button>
								<button type="button" onclick={() => void playTrackNext(track.id)}>Next</button>
								<button type="button" onclick={() => void addTrackToQueue(track.id)}>Queue</button>
							</div>
						</article>
					{/each}
				</div>
			{:else if mode === 'tidal' && tidalResults.length > 0}
				<div class="remote-search-results">
					{#each tidalResults as track (track.tidal_id)}
						<article class="remote-search-row">
							<div>
								<strong>{track.title}</strong>
								<span>{track.artist_name ?? 'Unknown artist'}</span>
							</div>
							<div class="remote-search-actions">
								<button type="button" onclick={() => void playTidalTrackNow(track)}>Play</button>
								<button type="button" onclick={() => void playTidalTrackNext(track)}>Next</button>
								<button type="button" onclick={() => void addTidalTrackToQueue(track)}>Queue</button>
							</div>
						</article>
					{/each}
				</div>
			{:else if shouldRunRemoteSearch(query)}
				<p class="remote-search-status">No {mode === 'tidal' ? 'TIDAL' : 'local'} matches.</p>
			{/if}
		</div>
	{/if}
</section>

<style>
	.remote-search,
	.remote-search-panel,
	.remote-search label,
	.remote-search-results,
	.remote-search-row {
		display: grid;
		gap: 10px;
	}

	.remote-search-tabs {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 8px;
	}

	.remote-search-tabs button,
	.remote-search-toggle,
	.remote-search input {
		min-height: 48px;
	}

	.remote-search-tabs button.active {
		background: var(--surface-2);
		color: var(--text-primary);
	}

	.remote-search-status {
		margin: 0;
		color: var(--text-muted);
	}

	.remote-search-row {
		padding: 10px 0;
		border-top: 1px solid var(--border-subtle);
	}

	.remote-search-row strong,
	.remote-search-row span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-search-actions {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 8px;
	}

	.remote-search-actions button {
		min-height: 42px;
	}
</style>
