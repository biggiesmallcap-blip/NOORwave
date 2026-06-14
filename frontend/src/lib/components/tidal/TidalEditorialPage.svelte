<script lang="ts">
	import { onDestroy, untrack } from 'svelte';
	import { ApiError, api, type TidalHomeModule } from '$lib/api/client';
	import { tidalStatus } from '$lib/stores/tidal';
	import { goBack } from '$lib/navigation/back';
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
		backFallback?: string;
		mediaKind?: 'audio' | 'video';
	};

	let {
		pagePath,
		eyebrow = 'TIDAL',
		title,
		subtitle,
		emptyText = 'TIDAL returned no editorial modules right now.',
		disconnectedText = 'Connect TIDAL to see this editorial page.',
		errorText = 'Could not load this TIDAL page.',
		backFallback = '/library',
		mediaKind = 'audio',
	}: Props = $props();

	let modules = $state<TidalHomeModule[]>([]);
	let viewState = $state<ViewState>('loading');
	let inFlight = false;
	let loadSeq = 0;
	let pendingPagePath = $state<string | null>(null);
	let loadedPagePath = $state<string | null>(null);

	onDestroy(() => {
		loadSeq += 1;
	});

	$effect(() => {
		const currentPagePath = pagePath;
		if ($tidalStatus !== 'connected') {
			loadSeq += 1;
			inFlight = false;
			pendingPagePath = null;
			loadedPagePath = null;
			modules = [];
			viewState = 'disconnected';
			return;
		}

		const pending = untrack(() => pendingPagePath);
		const loaded = untrack(() => loadedPagePath);
		const cur = untrack(() => viewState);
		if (pending !== currentPagePath && loaded !== currentPagePath) {
			void load(currentPagePath);
		} else if (pending !== currentPagePath && cur !== 'loading' && cur !== 'ready') {
			void load(currentPagePath);
		}
	});

	async function load(targetPagePath: string | null = null) {
		const requestedPagePath = targetPagePath ?? pagePath;
		if (inFlight && pendingPagePath === requestedPagePath) return;
		const seq = ++loadSeq;
		inFlight = true;
		pendingPagePath = requestedPagePath;
		if (modules.length === 0 || loadedPagePath !== requestedPagePath) {
			modules = [];
			viewState = 'loading';
		}
		try {
			const data = await api.getTidalPage(requestedPagePath);
			if (seq !== loadSeq || requestedPagePath !== pagePath) return;
			modules = data.modules ?? [];
			loadedPagePath = requestedPagePath;
			viewState = modules.length > 0 ? 'ready' : 'empty';
		} catch (e) {
			if (seq !== loadSeq || requestedPagePath !== pagePath) return;
			loadedPagePath = requestedPagePath;
			if (e instanceof ApiError && e.status === 503) {
				viewState = 'disconnected';
			} else {
				viewState = 'error';
			}
		} finally {
			if (seq === loadSeq) {
				inFlight = false;
				pendingPagePath = null;
			}
		}
	}
</script>

<svelte:head><title>{title} . NOOR</title></svelte:head>

<div class="page" data-tidal-editorial-page={pagePath}>
	<button class="back-link" type="button" onclick={() => goBack(backFallback)}>&lt; Back</button>
	<PageHeader {eyebrow} {title} {subtitle} variant="editorial" />

	{#if viewState === 'loading'}
		<p class="muted-line">Loading {title}...</p>
	{:else if viewState === 'ready'}
		<TidalDiscoverShelves {modules} {mediaKind} />
	{:else if viewState === 'empty'}
		<p class="muted-line">{emptyText} <button class="inline-link" onclick={() => void load()}>Retry</button></p>
	{:else if viewState === 'disconnected'}
		<p class="muted-line">{disconnectedText} <a class="inline-link" href="/settings#sources-tidal">Open settings</a></p>
	{:else if viewState === 'error'}
		<p class="muted-line">{errorText} <button class="inline-link" onclick={() => void load()}>Retry</button></p>
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

	.back-link {
		align-self: flex-start;
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		cursor: pointer;
	}

	.back-link:hover,
	.back-link:focus-visible {
		color: var(--text-primary);
		text-decoration: underline;
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
