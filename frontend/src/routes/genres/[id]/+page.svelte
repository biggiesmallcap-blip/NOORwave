<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { cachedApi } from '$lib/cache/api_queries';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import GenreDetail from '$lib/components/Genre/GenreDetail.svelte';
	import { buildGenreSummary, type GenreSnapshot } from '$lib/components/Genre/genreSummary';

	let genreId = $derived(Number(page.params.id));
	// Instant paint from the persisted galaxy snapshot, revalidated on mount.
	let snapshot = $state<GenreSnapshot | null>(cachedApi.genreGalaxySnapshotQuery(90).getSnapshot().data ?? null);
	let error = $state<string | null>(null);
	let summary = $derived(snapshot ? buildGenreSummary(snapshot, genreId) : null);

	onMount(() => {
		cachedApi
			.getGenreGalaxySnapshot(90)
			.then((next) => {
				snapshot = next;
				error = null;
			})
			.catch((reason) => {
				if (!snapshot) error = reason instanceof Error ? reason.message : String(reason);
			});
	});
</script>

<svelte:head>
	<title>{summary ? `${summary.name} | NOOR` : 'Genre | NOOR'}</title>
</svelte:head>

{#if summary}
	<GenreDetail node={summary} />
{:else if error}
	<EmptyState title="Genre unavailable" copy={error} />
{:else if snapshot}
	<EmptyState title="Genre not found" copy="This genre is no longer in your galaxy.">
		{#snippet actions()}
			<a class="btn btn-glass" href="/genres">Back to galaxy</a>
		{/snippet}
	</EmptyState>
{:else}
	<EmptyState title="Loading genre" copy="Reading your genre map." />
{/if}
