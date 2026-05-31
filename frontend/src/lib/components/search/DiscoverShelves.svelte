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

	const cachedOnMount = getCachedHomeModules();
	let modules = $state<TidalHomeModule[]>(cachedOnMount ?? []);
	let viewState = $state<State>(
		cachedOnMount && cachedOnMount.length > 0 ? 'ready' : 'loading'
	);

	onMount(() => {
		if (cachedOnMount && cachedOnMount.length > 0) return;
		void load();
	});

	$effect(() => {
		if ($tidalStatus !== 'connected') return;
		const cur = untrack(() => viewState);
		if (cur !== 'loading' && cur !== 'ready') {
			void load();
		}
	});

	async function load() {
		viewState = 'loading';
		try {
			const data = await cachedApi.getTidalHomeModules();
			modules = data.modules ?? [];
			if (modules.length > 0) putCachedHomeModules(modules);
			viewState = modules.length > 0 ? 'ready' : 'empty';
		} catch (e) {
			if (e instanceof ApiError && e.status === 503) {
				clearCachedHomeModules();
				viewState = 'disconnected';
			} else {
				viewState = 'error';
			}
		}
	}
</script>

{#if viewState === 'loading'}
	<p class="muted-line">Loading discover...</p>
{:else if viewState === 'ready'}
	<TidalDiscoverShelves {modules} />
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
