<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { ApiError, type TidalHomeModule } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import { tidalStatus } from '$lib/stores/tidal';
	import {
		getCachedHomeModules,
		putCachedHomeModules,
		clearCachedHomeModules
	} from '$lib/stores/tidal-home-modules-cache';
	import TidalDiscoverShelves from './TidalDiscoverShelves.svelte';

	type State = 'loading' | 'ready' | 'empty' | 'disconnected' | 'error';

	// `index` staggers the entrance when this sits in a stack of shelves.
	// `quiet` suppresses the loading/empty/disconnected/error lines: on Home
	// this is one section among many, and a status sentence wedged between two
	// working shelves reads as breakage rather than information. The /search
	// caller leaves it off, where the shelves are the whole view and an
	// explanation is the useful thing to show.
	let { index = 0, quiet = false }: { index?: number; quiet?: boolean } = $props();

	const cachedOnMount = getCachedHomeModules();
	let modules = $state<TidalHomeModule[]>(cachedOnMount ?? []);
	let viewState = $state<State>(
		cachedOnMount && cachedOnMount.length > 0 ? 'ready' : 'loading'
	);
	let loadSeq = 0;

	onMount(() => {
		if (!cachedOnMount || cachedOnMount.length === 0) void load();
		return () => { loadSeq += 1; };
	});

	$effect(() => {
		if ($tidalStatus !== 'connected') return;
		const cur = untrack(() => viewState);
		if (cur !== 'loading' && cur !== 'ready') {
			void load();
		}
	});

	async function load() {
		const seq = ++loadSeq;
		viewState = 'loading';
		try {
			const data = await cachedApi.getTidalHomeModules();
			if (seq !== loadSeq) return;
			const nextModules = data.modules ?? [];
			modules = nextModules;
			if (nextModules.length > 0) putCachedHomeModules(nextModules);
			viewState = nextModules.length > 0 ? 'ready' : 'empty';
		} catch (e) {
			if (seq !== loadSeq) return;
			if (e instanceof ApiError && e.status === 503) {
				clearCachedHomeModules();
				viewState = 'disconnected';
			} else {
				viewState = 'error';
			}
		}
	}
</script>

{#if viewState === 'ready'}
	<TidalDiscoverShelves {modules} startIndex={index} />
{:else if quiet}
	<!-- Nothing to show and nothing worth saying about it here. -->
{:else if viewState === 'loading'}
	<p class="muted-line">Loading discover...</p>
{:else if viewState === 'empty'}
	<p class="muted-line">
		TIDAL returned no editorial modules right now.
		<button class="inline-link" onclick={load}>Retry</button>
	</p>
{:else if viewState === 'disconnected'}
	<p class="muted-line">
		Connect TIDAL to see fresh discover picks.
		<a class="inline-link" href="/settings#sources-tidal">Open settings</a>
	</p>
{:else if viewState === 'error'}
	<p class="muted-line">
		Couldn't load discover.
		<button class="inline-link" onclick={load}>Retry</button>
	</p>
{/if}

<style>
	.muted-line {
		margin: 0;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
	}
	.inline-link {
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		color: var(--accent-line);
		cursor: pointer;
		text-decoration: underline;
		text-underline-offset: 2px;
		margin-left: var(--space-1);
	}
</style>
