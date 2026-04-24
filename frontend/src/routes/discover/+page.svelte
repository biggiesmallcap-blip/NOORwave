<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { currentTrack, automixEnabled, automixDiscoverNew, automixUseLearning } from '$lib/stores/player';
	import { discoverSpace, loadSpace } from '$lib/stores/discover_space';
	import DiscoverSpace from '$lib/components/Discover/DiscoverSpace.svelte';
	import DiscoverFilters from '$lib/components/Discover/DiscoverFilters.svelte';
	import DiscoverPanel from '$lib/components/Discover/DiscoverPanel.svelte';
	import PlaylistBuilder from '$lib/components/Discover/PlaylistBuilder.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import type { DiscoverTrackNode, DiscoverViewMode } from '$lib/components/Discover/discover.types';

	let selectedNodes = $state<DiscoverTrackNode[]>([]);
	let panelNode = $state<DiscoverTrackNode | null>(null);
	let searchQuery = $state('');
	let isSearching = $state(false);

	// Track what seed was used so we only reload when the track actually changes
	let loadedSeedId = $state<number | undefined>(undefined);

	function handleModeChange(mode: DiscoverViewMode) {
		const track = get(currentTrack);
		loadedSeedId = track?.id;
		loadSpace(mode, track?.id);
	}

	function handleHover(_node: DiscoverTrackNode | null) {}

	function handleSelect(node: DiscoverTrackNode) {
		selectedNodes = [...selectedNodes, node];
		panelNode = node;
	}

	function handleNewNodes(incoming: DiscoverTrackNode[]) {
		discoverSpace.update(s => ({ ...s, nodes: [...s.nodes, ...incoming] }));
	}

	async function handleSearch(e: Event) {
		e.preventDefault();
		const q = searchQuery.trim();
		if (!q) return;
		isSearching = true;
		const fn = (window as any).__discoverSpaceHyperspaceSearch;
		if (fn) await fn(q);
		isSearching = false;
		searchQuery = '';
	}

	// Re-seed the space when the playing track changes (only if the space was
	// already seeded by a different track — avoids reloading on every automix tick)
	$effect(() => {
		const track = $currentTrack;
		if (track && track.id !== loadedSeedId && $discoverSpace.nodes.length > 0) {
			loadedSeedId = track.id;
			loadSpace($discoverSpace.mode, track.id);
		}
	});

	onMount(() => {
		const track = get(currentTrack);
		loadedSeedId = track?.id;
		loadSpace('radio', track?.id);
	});
</script>

<div class="discover-page">
	<div class="discover-header">
		<div class="header-text">
			<span class="eyebrow">Sound Space</span>
			<h1>Navigate your music by feel.</h1>
		</div>
		<form class="search-form" onsubmit={handleSearch}>
			<input
				class="search-input"
				type="text"
				placeholder="Jump to… dark ambient, 140bpm drum & bass…"
				bind:value={searchQuery}
				disabled={isSearching}
			/>
			<button class="search-btn" type="submit" disabled={isSearching || !searchQuery.trim()}>
				{isSearching ? '⟳' : '⤑'}
			</button>
		</form>
	</div>

	{#if $automixEnabled}
		<div class="automix-bar">
			<span class="automix-dot"></span>
			<span class="automix-label">Automix active</span>
			{#if $automixDiscoverNew}<span class="automix-tag">Discover</span>{/if}
			{#if $automixUseLearning}<span class="automix-tag">Learning</span>{/if}
			{#if $currentTrack}
				<span class="automix-seed">seeded from <strong>{$currentTrack.title}</strong></span>
			{/if}
		</div>
	{/if}

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
					currentTrackId={$currentTrack?.id ?? null}
					onHover={handleHover}
					onSelect={handleSelect}
					onNewNodes={handleNewNodes}
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
		gap: var(--space-3, 12px);
	}

	.discover-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4, 16px);
		padding: 0 var(--space-3, 12px);
		flex-shrink: 0;
	}
	.header-text {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.eyebrow {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: rgba(255,255,255,0.35);
	}
	h1 {
		font-size: 1.1rem;
		font-weight: 600;
		color: rgba(255,255,255,0.9);
		margin: 0;
	}

	.search-form {
		display: flex;
		align-items: center;
		gap: 6px;
		flex: 1;
		max-width: 480px;
	}
	.search-input {
		flex: 1;
		padding: 8px 14px;
		border-radius: 10px;
		border: 1px solid rgba(255,255,255,0.1);
		background: rgba(255,255,255,0.05);
		color: rgba(255,255,255,0.9);
		font-size: 0.85rem;
		outline: none;
		transition: border-color 0.15s;
	}
	.search-input::placeholder {
		color: rgba(255,255,255,0.3);
	}
	.search-input:focus {
		border-color: rgba(124,128,255,0.5);
		background: rgba(124,128,255,0.07);
	}
	.search-input:disabled {
		opacity: 0.5;
	}
	.search-btn {
		padding: 8px 14px;
		border-radius: 10px;
		border: none;
		background: rgba(124,128,255,0.2);
		color: rgba(255,255,255,0.85);
		cursor: pointer;
		font-size: 1rem;
		transition: background 0.15s;
	}
	.search-btn:hover:not(:disabled) {
		background: rgba(124,128,255,0.35);
	}
	.search-btn:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.automix-bar {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 6px 14px;
		border-radius: 10px;
		background: rgba(124,128,255,0.08);
		border: 1px solid rgba(124,128,255,0.18);
		flex-shrink: 0;
		font-size: 0.78rem;
		color: rgba(255,255,255,0.6);
		margin: 0 var(--space-3, 12px);
	}
	.automix-dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: rgba(124,128,255,1);
		box-shadow: 0 0 6px rgba(124,128,255,0.8);
		flex-shrink: 0;
	}
	.automix-label { color: rgba(255,255,255,0.8); font-weight: 500; }
	.automix-tag {
		padding: 2px 7px;
		border-radius: 999px;
		background: rgba(124,128,255,0.15);
		color: rgba(124,128,255,1);
		font-size: 0.72rem;
		font-weight: 600;
		letter-spacing: 0.04em;
	}
	.automix-seed {
		margin-left: auto;
		color: rgba(255,255,255,0.35);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 260px;
	}
	.automix-seed strong { color: rgba(255,255,255,0.6); font-weight: 500; }

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
		height: 100%;
	}
</style>
