<script lang="ts">
	import { api, type Track } from '$lib/api/client';
	import { addTrackToQueue, playTrackNext, playTrackNow } from '$lib/stores/player';
	import { normalizeRemoteSearchQuery, shouldRunRemoteSearch } from '$lib/remote/search';

	let query = $state('');
	let open = $state(false);
	let busy = $state(false);
	let error = $state('');
	let results = $state<Track[]>([]);
	let searchSeq = 0;

	$effect(() => {
		const normalized = normalizeRemoteSearchQuery(query);
		if (!shouldRunRemoteSearch(normalized)) {
			results = [];
			error = '';
			busy = false;
			return;
		}
		const seq = ++searchSeq;
		busy = true;
		const timer = setTimeout(() => {
			void api.search(normalized, 8)
				.then((data) => {
					if (seq !== searchSeq) return;
					results = data.tracks;
					error = '';
				})
				.catch(() => {
					if (seq !== searchSeq) return;
					results = [];
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
			<label>
				<span>Search library</span>
				<input bind:value={query} type="search" inputmode="search" autocomplete="off" placeholder="Track, artist, album" />
			</label>

			{#if busy}
				<p class="remote-search-status">Searching...</p>
			{:else if error}
				<p class="remote-search-status">{error}</p>
			{:else if results.length > 0}
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
			{:else if shouldRunRemoteSearch(query)}
				<p class="remote-search-status">No local matches.</p>
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

	.remote-search-toggle,
	.remote-search input {
		min-height: 48px;
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
