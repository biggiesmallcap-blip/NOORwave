<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { currentTrack } from '$lib/stores/player';
	import { discoverSpace, loadSpace } from '$lib/stores/discover_space';
	import DiscoverSpace from '$lib/components/Discover/DiscoverSpace.svelte';
	import DiscoverFilters from '$lib/components/Discover/DiscoverFilters.svelte';
	import DiscoverPanel from '$lib/components/Discover/DiscoverPanel.svelte';
	import PlaylistBuilder from '$lib/components/Discover/PlaylistBuilder.svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import type { DiscoverTrackNode, DiscoverViewMode } from '$lib/components/Discover/discover.types';

	let selectedNodes = $state<DiscoverTrackNode[]>([]);
	let panelNode = $state<DiscoverTrackNode | null>(null);

	function handleModeChange(mode: DiscoverViewMode) {
		const track = get(currentTrack);
		loadSpace(mode, track?.id);
	}

	function handleHover(node: DiscoverTrackNode | null) {
		// Optional: update panel on hover
	}

	function handleSelect(node: DiscoverTrackNode) {
		selectedNodes = [...selectedNodes, node];
		panelNode = node;
	}

	onMount(() => {
		const track = get(currentTrack);
		loadSpace('radio', track?.id);
	});
</script>

<div class="discover-page">
	<PageHeader
		eyebrow="Sound Space"
		title="Navigate your music by feel."
		subtitle="Radio, exploration, harmonic relationships, energy arcs, and sample discovery — all in one canvas."
	/>

	<div class="discover-layout">
		<div class="discover-sidebar">
			<DiscoverFilters
				mode={$discoverSpace.mode}
				onModeChange={handleModeChange}
			/>
			<PlaylistBuilder {selectedNodes} />
		</div>

		<div class="discover-main">
			{#if $discoverSpace.loading && $discoverSpace.nodes.length === 0}
				<EmptyState title="Mapping your sound space" copy="This takes a moment on first load." />
			{:else if $discoverSpace.nodes.length === 0}
				<EmptyState title="No tracks found" copy="Try a different mode or seed track." />
			{:else}
				<DiscoverSpace
					nodes={$discoverSpace.nodes}
					edges={$discoverSpace.edges}
					mode={$discoverSpace.mode}
					onHover={handleHover}
					onSelect={handleSelect}
				/>
			{/if}
		</div>

		<div class="discover-sidebar right">
			<DiscoverPanel node={panelNode} />
		</div>
	</div>
</div>

<style>
	.discover-page {
		height: calc(100vh - 80px);
		display: flex;
		flex-direction: column;
	}
	.discover-layout {
		flex: 1;
		display: grid;
		grid-template-columns: 200px 1fr 280px;
		gap: var(--space-3, 12px);
		min-height: 0;
	}
	.discover-sidebar {
		display: flex;
		flex-direction: column;
		gap: var(--space-3, 12px);
		overflow-y: auto;
	}
	.discover-sidebar.right {
		border-left: 1px solid rgba(255,255,255,0.06);
	}
	.discover-main {
		position: relative;
		min-height: 0;
	}
	.discover-main > :global(.glass-panel) {
		height: 100%;
	}
</style>
