<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { ApiError, api, type TidalHomeModule } from '$lib/api/client';
	import { tidalStatus } from '$lib/stores/tidal';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import TidalDiscoverShelves from '$lib/components/search/TidalDiscoverShelves.svelte';

	type ViewState = 'loading' | 'ready' | 'empty' | 'disconnected' | 'error';

	type Props = {
		pagePath: string;
		eyebrow?: string;
		title: string;
		subtitle: string;
		emptyText?: string;
		disconnectedText?: string;
		errorText?: string;
	};

	let {
		pagePath,
		eyebrow = 'TIDAL',
		title,
		subtitle,
		emptyText = 'TIDAL returned no editorial modules right now.',
		disconnectedText = 'Connect TIDAL to see this editorial page.',
		errorText = 'Could not load this TIDAL page.',
	}: Props = $props();

	let modules = $state<TidalHomeModule[]>([]);
	let viewState = $state<ViewState>('loading');
	let inFlight = false;
	let loadSeq = 0;

	onMount(() => {
		void load();
		return () => { loadSeq += 1; };
	});

	$effect(() => {
		if ($tidalStatus !== 'connected') return;
		const cur = untrack(() => viewState);
		if (cur !== 'loading' && cur !== 'ready') void load();
	});

	async function load() {
		if (inFlight) return;
		const seq = ++loadSeq;
		inFlight = true;
		if (modules.length === 0) viewState = 'loading';
		try {
			const data = await api.getTidalPage(pagePath);
			if (seq !== loadSeq) return;
			modules = data.modules ?? [];
			viewState = modules.length > 0 ? 'ready' : 'empty';
		} catch (e) {
			if (seq !== loadSeq) return;
			if (e instanceof ApiError && e.status === 503) {
				viewState = 'disconnected';
			} else {
				viewState = 'error';
			}
		} finally {
			if (seq === loadSeq) inFlight = false;
		}
	}
</script>

<svelte:head><title>{title} . NOOR</title></svelte:head>

<div class="page" data-tidal-editorial-page={pagePath}>
	<PageHeader {eyebrow} {title} {subtitle} variant="editorial" />

	{#if viewState === 'loading'}
		<p class="muted-line">Loading {title}...</p>
	{:else if viewState === 'ready'}
		<TidalDiscoverShelves {modules} />
	{:else if viewState === 'empty'}
		<p class="muted-line">{emptyText} <button class="inline-link" onclick={load}>Retry</button></p>
	{:else if viewState === 'disconnected'}
		<p class="muted-line">{disconnectedText} <a class="inline-link" href="/settings#sources-tidal">Open settings</a></p>
	{:else if viewState === 'error'}
		<p class="muted-line">{errorText} <button class="inline-link" onclick={load}>Retry</button></p>
	{/if}
</div>

<style>
	.page {
		max-width: var(--content-width);
		margin: 0 auto;
		padding: var(--space-5) var(--space-4) var(--space-7);
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}

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
