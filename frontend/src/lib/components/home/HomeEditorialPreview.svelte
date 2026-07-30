<script lang="ts">
	import { onMount } from 'svelte';
	import { type TidalHomeModule } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import TidalDiscoverShelves from '$lib/components/search/TidalDiscoverShelves.svelte';

	// A few shelves off a TIDAL editorial page, with the header linking to the
	// full route. Those routes (/new-releases, /hires, /explore) already exist
	// and are built on TidalEditorialPage, but nothing in the app links to them,
	// so they were unreachable. This is the entry point.
	//
	// TidalEditorialPage itself is not reusable here: it owns a back link, a
	// PageHeader and the page padding, all of which are wrong inside a shelf
	// stack. Only the fetch and the module rendering are shared, and the
	// rendering is TidalDiscoverShelves, which takes modules as a prop.
	let {
		pagePath,
		title,
		href,
		eyebrow = 'TIDAL',
		limitModules = 2,
		index = 0,
	}: {
		pagePath: string;
		title: string;
		href: string;
		eyebrow?: string;
		limitModules?: number;
		index?: number;
	} = $props();

	let modules = $state<TidalHomeModule[]>([]);

	// Home is a browse surface, not a diagnostics page: a TIDAL outage or a
	// disconnected account should make this section disappear, not print an
	// error between two working shelves. The dedicated route still explains
	// itself properly when the user follows the link.
	onMount(() => {
		void (async () => {
			try {
				const data = await cachedApi.getTidalPage(pagePath);
				modules = (data.modules ?? []).slice(0, limitModules);
			} catch {
				modules = [];
			}
		})();
	});
</script>

{#if modules.length > 0}
	<section
		class="editorial-preview rise-in-shelf"
		data-section={`editorial-${pagePath}`}
		style={`--rise-index: ${index}`}
	>
		<SectionHeader {eyebrow} {title} variant="charts" level={2} {href} linkLabel="See all" />
		<TidalDiscoverShelves {modules} nested />
	</section>
{/if}

<style>
	.editorial-preview {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}
</style>
