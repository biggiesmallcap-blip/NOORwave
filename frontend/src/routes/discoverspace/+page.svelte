<script lang="ts">
	import { onMount } from 'svelte';
	import { currentTrack, automixEnabled, automixDiscoverNew, automixUseLearning } from '$lib/stores/player';
	import {
		discoverSpaceStore,
		loadSpace,
		loadBlendSpace,
		addBlendSeed,
		removeBlendSeed,
		clearBlend,
		addBlendDiscoveries,
		playBlendDiscoveries,
		makeBlendRadio,
		lockSeed,
		unlockSeed,
		hydrateDiscoverControls,
		refreshSuggestedCoherence,
	} from '$lib/components/DiscoverSpace/discover_space_store';
	import DiscoverFilterBar from '$lib/components/DiscoverSpace/DiscoverFilterBar.svelte';
	import DiscoverRankedList from '$lib/components/DiscoverSpace/DiscoverRankedList.svelte';
	import DiscoverBreadcrumb from '$lib/components/DiscoverSpace/DiscoverBreadcrumb.svelte';
	import DiscoverSpace from '$lib/components/DiscoverSpace/DiscoverSpace.svelte';
	import DiscoverHoverCard from '$lib/components/DiscoverSpace/DiscoverHoverCard.svelte';
	import DiscoverSidePanel from '$lib/components/DiscoverSpace/DiscoverSidePanel.svelte';
	import DiscoverLensControl from '$lib/components/DiscoverSpace/DiscoverLensControl.svelte';
	import DiscoverTrainingStrip from '$lib/components/DiscoverSpace/DiscoverTrainingStrip.svelte';
	import DiscoverHelp from '$lib/components/DiscoverSpace/DiscoverHelp.svelte';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import { palette } from '$lib/stores/palette';
	import { applyPaletteTheme } from '$lib/components/wallpaper/paletteTheme';
	import type { DiscoverTrackNode, RadioMode } from '$lib/components/DiscoverSpace/discover_space_types';
	import { PAGE_TITLE, PAGE_SUBTITLE, SEARCH_PLACEHOLDER, EMPTY_STATE } from '$lib/components/DiscoverSpace/discover_space_story';

	// ── Seed resolution (mirrors /discover pattern) ───────────────────────────
	let resolvedSeedId = $derived(
		$discoverSpaceStore.lockedSeedId ?? $currentTrack?.id ?? null
	);
	let resolvedSeedSource = $derived<'locked' | 'playing' | null>(
		$discoverSpaceStore.lockedSeedId !== null ? 'locked'
		: $currentTrack?.id != null ? 'playing'
		: null
	);

	// Track last-loaded seed to avoid refetching on every reactive tick
	let lastLoadedSeedId = $state<number | null>(null);

	// The seed node object (for the side panel idle state)
	let anchorNode = $derived(
		$discoverSpaceStore.nodes.find((n) => n.isSeed || n.trackId === resolvedSeedId) ?? null
	);

	// ── Interaction state ─────────────────────────────────────────────────────
	let hoveredNode = $state<DiscoverTrackNode | null>(null);
	let hoverX = $state(0);
	let hoverY = $state(0);
	let selectedNode = $state<DiscoverTrackNode | null>(null);
	let playlistNodes = $state<DiscoverTrackNode[]>([]);
	let searchQuery = $state('');
	let isSearching = $state(false);
	let spaceCanvas: DiscoverSpace | undefined = $state();
	// The canvas is a dark stage in every theme. Scope the dark token set, with
	// this palette's dark ground and accents, to the canvas area so its overlays
	// stay legible in light mode (same pattern as the Genres night sky).
	let stageEl = $state<HTMLDivElement | null>(null);
	$effect(() => {
		if (stageEl) applyPaletteTheme(stageEl, $palette, 'dark');
	});
	let blendAction = $state<'add' | 'play' | 'radio' | null>(null);
	let lastNodeSignature = '';

	let canRunBlendActions = $derived(
		!$discoverSpaceStore.blendLoading
		&& ($discoverSpaceStore.blendHealth?.playable_external_count ?? 0) > 0
	);

	$effect(() => {
		const nodes = $discoverSpaceStore.nodes;
		const signature = nodes.map((node) => node.id).join('|');
		if (signature === lastNodeSignature) return;
		lastNodeSignature = signature;
		if (selectedNode) {
			selectedNode = nodes.find((node) => node.id === selectedNode?.id) ?? null;
		}
		if (hoveredNode) {
			hoveredNode = nodes.find((node) => node.id === hoveredNode?.id) ?? null;
			if (!hoveredNode) {
				hoverX = 0;
				hoverY = 0;
			}
		}
	});

	function handleModeChange(mode: RadioMode) {
		lastLoadedSeedId = resolvedSeedId;
		if (resolvedSeedId !== null) {
			loadSpace(mode, resolvedSeedId, undefined, resolvedSeedSource, $currentTrack?.id ?? null);
		} else {
			discoverSpaceStore.update((s) => ({ ...s, mode }));
		}
	}

	function handleHoverPosition(node: DiscoverTrackNode | null, x: number, y: number) {
		hoveredNode = node;
		hoverX = x;
		hoverY = y;
	}

	function handleSelectNode(node: DiscoverTrackNode | null) {
		selectedNode = node;
	}

	function handleAddToPlaylist(node: DiscoverTrackNode) {
		discoverSpaceStore.update((s) => ({
			...s,
			nodes: s.nodes.map((n) =>
				n.trackId === node.trackId ? { ...n, inPlaylistBuilder: !n.inPlaylistBuilder } : n
			),
		}));
		if (!playlistNodes.some((n) => n.trackId === node.trackId)) {
			playlistNodes = [...playlistNodes, node];
		} else {
			playlistNodes = playlistNodes.filter((n) => n.trackId !== node.trackId);
		}
	}

	function handleAddToBlend(node: DiscoverTrackNode) {
		// The store triggers the blend fetch itself when 2+ seeds exist.
		addBlendSeed(node);
	}

	function handleRemoveBlendSeed(identity: string) {
		removeBlendSeed(identity);
	}

	function handleClearBlend() {
		clearBlend();
		hoveredNode = null;
		selectedNode = null;
		if (resolvedSeedId !== null) {
			lastLoadedSeedId = resolvedSeedId;
			loadSpace($discoverSpaceStore.mode, resolvedSeedId, undefined, resolvedSeedSource, $currentTrack?.id ?? null);
		}
	}

	async function runBlendAction(action: 'add' | 'play' | 'radio') {
		if (blendAction !== null) return;
		blendAction = action;
		try {
			if (action === 'add') await addBlendDiscoveries();
			else if (action === 'play') await playBlendDiscoveries();
			else await makeBlendRadio();
		} finally {
			blendAction = null;
		}
	}

	function handleToggleLock() {
		if ($discoverSpaceStore.lockedSeedId !== null) {
			unlockSeed();
		} else {
			const trackId = $currentTrack?.id;
			if (trackId != null) lockSeed(trackId);
		}
	}

	async function handleSearch(e: Event) {
		e.preventDefault();
		const q = searchQuery.trim();
		if (!q) return;
		isSearching = true;
		// Load new space via store then trigger the canvas warp animation.
		await loadSpace(
			$discoverSpaceStore.mode,
			resolvedSeedId ?? undefined,
			q,
			resolvedSeedSource,
			$currentTrack?.id ?? null
		);
		await spaceCanvas?.hyperspaceSearch(q);
		isSearching = false;
		searchQuery = '';
	}

	// ── Auto-seed: refetch when resolved seed changes ─────────────────────────
	$effect(() => {
		const seedId = resolvedSeedId;
		if (seedId !== null && seedId !== lastLoadedSeedId) {
			lastLoadedSeedId = seedId;
			loadSpace($discoverSpaceStore.mode, seedId, undefined, resolvedSeedSource, $currentTrack?.id ?? null);
		}
	});

	onMount(() => {
		// Controls (coherence, filters, session id) hydrate before the first
		// load so the initial request already carries them.
		hydrateDiscoverControls();
		void refreshSuggestedCoherence();
		const seedId = resolvedSeedId;
		if (seedId !== null) {
			lastLoadedSeedId = seedId;
			loadSpace('radio', seedId, undefined, resolvedSeedSource, $currentTrack?.id ?? null);
		}
	});
</script>

<svelte:window onmouseleave={() => { hoveredNode = null; }} />

<div class="discoverspace-page">
	<!-- Header -->
	<div class="page-header">
		<div class="header-text">
			<h1 class="t-page-title">{PAGE_TITLE}</h1>
			<p class="tagline">{PAGE_SUBTITLE}</p>
		</div>
		<form class="search-form" onsubmit={handleSearch}>
			<SearchField
				bind:value={searchQuery}
				variant="page"
				size="sm"
				fill
				disabled={isSearching}
				placeholder={SEARCH_PLACEHOLDER}
			/>
			<button class="search-btn" type="submit" disabled={isSearching || !searchQuery.trim()}>
				{isSearching ? '⟳' : '⤑'}
			</button>
		</form>
		<DiscoverHelp />
	</div>

	<!-- Seed pill -->
	{#if $discoverSpaceStore.activeSeedId !== null}
		<div class="seed-pill">
			<span class="seed-source">
				{$discoverSpaceStore.activeSeedSource === 'locked' ? '🔒 Locked seed' : '▶ Auto-seeded from playing'}
			</span>
			{#if $currentTrack && $currentTrack.id === $discoverSpaceStore.activeSeedId}
				<span class="seed-title">{$currentTrack.title}</span>
			{/if}
			<button
				class="seed-toggle"
				onclick={handleToggleLock}
				disabled={$currentTrack?.id == null && $discoverSpaceStore.lockedSeedId === null}
			>
				{$discoverSpaceStore.lockedSeedId !== null ? 'Unlock' : 'Lock seed'}
			</button>
		</div>
	{/if}

	<DiscoverFilterBar />

	<DiscoverBreadcrumb />

	{#if $discoverSpaceStore.blendSeeds.length > 0}
		<div class="blend-strip" aria-label="Blend seeds">
			<div class="blend-seeds">
				<span class="blend-label">Blend</span>
				{#each $discoverSpaceStore.blendSeeds as seed (seed.identity)}
					<button
						class="blend-chip"
						type="button"
						onclick={() => handleRemoveBlendSeed(seed.identity)}
						aria-label="Remove {seed.title ?? seed.artist ?? seed.identity} from blend"
					>
						<span class="blend-chip-title">{seed.title ?? seed.artist ?? seed.identity}</span>
						<span class="blend-chip-remove" aria-hidden="true">x</span>
					</button>
				{/each}
			</div>
			<div class="blend-health">
				<span>{($discoverSpaceStore.blendHealth?.playable_external_count ?? 0)} ready</span>
				<span>{($discoverSpaceStore.blendHealth?.pending_external_count ?? 0)} pending</span>
				{#if $discoverSpaceStore.blendHealth}
					<span>{Math.round($discoverSpaceStore.blendHealth.coverage_ratio * 100)}% coverage</span>
				{/if}
			</div>
			<div class="blend-actions">
				<button
					class="blend-action"
					type="button"
					onclick={() => loadBlendSpace($currentTrack?.id ?? null)}
					disabled={$discoverSpaceStore.blendSeeds.length < 2 || $discoverSpaceStore.blendLoading}
				>
					{$discoverSpaceStore.blendLoading ? 'Loading' : 'Map blend'}
				</button>
				<button class="blend-action" type="button" onclick={() => runBlendAction('add')} disabled={!canRunBlendActions || blendAction !== null || $discoverSpaceStore.blendLoading}>Add discoveries</button>
				<button class="blend-action primary" type="button" onclick={() => runBlendAction('play')} disabled={!canRunBlendActions || blendAction !== null || $discoverSpaceStore.blendLoading}>Play discoveries</button>
				<button class="blend-action" type="button" onclick={() => runBlendAction('radio')} disabled={!canRunBlendActions || blendAction !== null || $discoverSpaceStore.blendLoading}>Make blend radio</button>
				<button class="blend-action subtle" type="button" onclick={handleClearBlend}>Clear blend</button>
			</div>
		</div>
	{/if}

	<!-- Automix bar -->
	{#if $automixEnabled}
		<div class="automix-bar">
			<span class="automix-dot" aria-hidden="true"></span>
			<span class="automix-label">Automix active</span>
			{#if $automixDiscoverNew}<span class="automix-tag">Discover</span>{/if}
			{#if $automixUseLearning}<span class="automix-tag">Learning</span>{/if}
			{#if $currentTrack}
				<span class="automix-seed">seeded from <strong>{$currentTrack.title}</strong></span>
			{/if}
		</div>
	{/if}

	<!-- Main layout -->
	<div class="page-layout">
		<!-- Canvas area -->
		<div class="canvas-area" data-theme="dark" bind:this={stageEl}>
			<!-- Empty / loading states -->
			{#if resolvedSeedId === null}
				<div class="empty-state">
					<div class="empty-icon" aria-hidden="true">◈</div>
					<div class="empty-title">{EMPTY_STATE.noSeed.title}</div>
					<div class="empty-copy">{EMPTY_STATE.noSeed.copy}</div>
				</div>
			{:else if $discoverSpaceStore.loading && $discoverSpaceStore.nodes.length === 0}
				<div class="empty-state">
					<div class="empty-icon spinning" aria-hidden="true">◈</div>
					<div class="empty-title">{EMPTY_STATE.loading.title}</div>
					<div class="empty-copy">{EMPTY_STATE.loading.copy}</div>
				</div>
			{:else if $discoverSpaceStore.error && $discoverSpaceStore.nodes.length === 0}
				<div class="empty-state">
					<div class="empty-icon" aria-hidden="true">!</div>
					<div class="empty-title">{EMPTY_STATE.loadFailed.title}</div>
					<div class="empty-copy">{EMPTY_STATE.loadFailed.copy}</div>
					<button
						class="retry-btn"
						onclick={() => resolvedSeedId && loadSpace($discoverSpaceStore.mode, resolvedSeedId, undefined, resolvedSeedSource, $currentTrack?.id ?? null)}
					>Retry</button>
				</div>
			{:else if $discoverSpaceStore.nodes.length === 0}
				<div class="empty-state">
					<div class="empty-icon" aria-hidden="true">◈</div>
					<div class="empty-title">{EMPTY_STATE.noTracks.title}</div>
					<div class="empty-copy">{EMPTY_STATE.noTracks.copy}</div>
				</div>
			{:else}
				<DiscoverSpace
					bind:this={spaceCanvas}
					currentTrackId={$currentTrack?.id ?? null}
					seedTrackId={$discoverSpaceStore.activeSeedId}
					isLocked={$discoverSpaceStore.lockedSeedId !== null}
					onHoverNode={handleHoverPosition}
					onSelectNode={handleSelectNode}
				/>
			{/if}

			<!-- Lens control floats over canvas (top-left) -->
			{#if $discoverSpaceStore.nodes.length > 0}
				<div class="canvas-overlay top-left">
					<DiscoverLensControl />
				</div>
			{/if}

			<!-- Training strip bottom-center -->
			<div class="canvas-overlay bottom-center">
				<DiscoverTrainingStrip />
			</div>

			<!-- Seed refresh progress pill (top-right while computing) -->
			{#if $discoverSpaceStore.refreshProgress !== null}
				{@const rp = $discoverSpaceStore.refreshProgress}
				<div class="canvas-overlay top-right">
					<div class="refresh-pill">
						<span class="refresh-spinner" aria-hidden="true"></span>
						<span class="refresh-label">
							{#if rp.stage === 'loading'}Loading embeddings{:else if rp.stage === 'computing'}Computing similarity{:else if rp.stage === 'saving'}Saving connections{:else}{rp.stage}{/if}
						</span>
						<span class="refresh-pct">{Math.round(rp.progress * 100)}%</span>
					</div>
				</div>
			{/if}
		</div>

		<!-- Ranked list (collapsible middle column) -->
		<DiscoverRankedList onSelectNode={(node) => handleSelectNode(node)} />

		<!-- Side panel -->
		<DiscoverSidePanel
			node={selectedNode}
			seedNode={anchorNode}
			onAddToPlaylist={handleAddToPlaylist}
			onAddToBlend={handleAddToBlend}
		/>
	</div>

	<!-- Hover card -->
	<DiscoverHoverCard
		node={hoveredNode}
		mouseX={hoverX}
		mouseY={hoverY}
		seedTrackId={$discoverSpaceStore.activeSeedId}
		isLocked={$discoverSpaceStore.lockedSeedId !== null}
	/>
</div>

<style>
	.discoverspace-page {
		height: calc(100vh - 80px);
		display: flex;
		flex-direction: column;
		gap: var(--space-3, 12px);
		overflow: hidden;
	}

	.page-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4, 16px);
		flex-shrink: 0;
	}
	.header-text { display: flex; flex-direction: column; gap: 2px; }
	.tagline {
		margin: 0;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
	}
	h1 {
		margin: 0;
		color: var(--text-primary);
	}

	.search-form {
		display: flex;
		align-items: center;
		gap: 6px;
		flex: 1;
		max-width: 480px;
	}
	.search-btn {
		padding: 8px 14px;
		border-radius: var(--radius-md);
		border: 1px solid var(--accent-line);
		background: var(--accent-soft);
		color: var(--text-primary);
		cursor: pointer;
		font-size: var(--font-size-md);
		transition: background var(--motion-fast), border-color var(--motion-fast);
	}
	.search-btn:hover:not(:disabled) { background: var(--accent-line); border-color: var(--accent); }
	.search-btn:disabled { opacity: 0.4; cursor: default; }

	.seed-pill {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 8px 14px;
		margin: 0;
		border-radius: 999px;
		background: var(--accent-soft);
		border: 1px solid var(--accent-line);
		font-size: var(--font-size-xs);
		width: fit-content;
		flex-shrink: 0;
		margin: 0 var(--space-3, 12px);
	}
	.seed-source { color: var(--text-secondary); letter-spacing: 0.04em; }
	.seed-title { font-weight: var(--font-weight-semibold); color: var(--text-primary); }
	.seed-toggle {
		margin-left: auto;
		background: transparent;
		border: 1px solid var(--accent-line);
		color: var(--accent-strong);
		border-radius: 999px;
		padding: 4px 12px;
		font-size: var(--font-size-xs);
		cursor: pointer;
		transition: background var(--motion-fast), color var(--motion-fast);
	}
	.seed-toggle:hover:not(:disabled) { background: var(--accent-soft); color: var(--text-primary); }
	.seed-toggle:disabled { opacity: 0.4; cursor: not-allowed; }

	.automix-bar {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 6px 14px;
		border-radius: 10px;
		background: var(--accent-soft);
		border: 1px solid var(--accent-line);
		flex-shrink: 0;
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		margin: 0 var(--space-3, 12px);
	}
	.automix-dot {
		width: 7px; height: 7px; border-radius: 50%;
		background: var(--accent);
		box-shadow: 0 0 6px var(--accent-glow);
		flex-shrink: 0;
	}
	.automix-label { color: var(--text-primary); font-weight: var(--font-weight-medium); }
	.automix-tag {
		padding: 2px 7px;
		border-radius: 999px;
		background: var(--accent-soft);
		color: var(--accent-strong);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.04em;
	}
	.automix-seed {
		margin-left: auto;
		color: var(--text-tertiary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 260px;
	}
	.automix-seed strong { color: var(--text-secondary); font-weight: var(--font-weight-medium); }

	.blend-strip {
		display: flex;
		align-items: center;
		gap: 10px;
		margin: 0 var(--space-3, 12px);
		padding: 8px 10px;
		border: 1px solid var(--border-subtle);
		border-radius: 10px;
		background: var(--bg-surface);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		flex-shrink: 0;
	}
	.blend-seeds {
		display: flex;
		align-items: center;
		gap: 6px;
		min-width: 0;
		flex: 1;
	}
	.blend-label {
		color: var(--accent-strong);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}
	.blend-chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		max-width: 180px;
		min-height: 28px;
		padding: 4px 8px;
		border: 1px solid var(--accent-line);
		border-radius: 999px;
		background: var(--accent-soft);
		color: var(--text-primary);
		cursor: pointer;
	}
	.blend-chip:hover { background: var(--accent-line); }
	.blend-chip-title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.blend-chip-remove { color: var(--text-tertiary); }
	.blend-health {
		display: flex;
		align-items: center;
		gap: 8px;
		color: var(--text-tertiary);
		white-space: nowrap;
	}
	.blend-actions {
		display: flex;
		align-items: center;
		gap: 6px;
		flex-wrap: wrap;
		justify-content: flex-end;
	}
	.blend-action {
		min-height: 30px;
		padding: 5px 10px;
		border-radius: 8px;
		border: 1px solid var(--border-muted);
		background: var(--bg-surface);
		color: var(--text-secondary);
		cursor: pointer;
	}
	.blend-action.primary {
		border-color: var(--accent);
		background: var(--accent);
		color: var(--text-on-accent);
	}
	.blend-action.subtle { color: var(--text-tertiary); }
	.blend-action:hover:not(:disabled) { background: var(--bg-hover); }
	.blend-action.primary:hover:not(:disabled) { background: var(--accent); filter: brightness(1.1); }
	.blend-action:disabled {
		opacity: 0.42;
		cursor: not-allowed;
	}

	.page-layout {
		flex: 1;
		display: grid;
		/* canvas | collapsible ranked list | side panel */
		grid-template-columns: 1fr auto 280px;
		gap: var(--space-3, 12px);
		min-height: 0;
		padding: 0 var(--space-3, 12px);
		padding-bottom: var(--space-3, 12px);
	}

	.canvas-area {
		position: relative;
		min-height: 0;
		height: 100%;
		border-radius: 12px;
		overflow: hidden;
		background: var(--bg-base);
		color: var(--text-primary);
		border: 1px solid var(--border-subtle);
	}

	.canvas-overlay {
		position: absolute;
		z-index: 10;
		pointer-events: auto;
	}
	.canvas-overlay.top-left { top: 12px; left: 12px; }
	.canvas-overlay.top-right { top: 12px; right: 12px; }
	.canvas-overlay.bottom-center {
		bottom: 12px;
		left: 50%;
		transform: translateX(-50%);
	}

	.refresh-pill {
		display: flex;
		align-items: center;
		gap: 7px;
		padding: 5px 10px 5px 8px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--bg-surface-strong) 88%, transparent);
		border: 1px solid var(--accent-line);
		backdrop-filter: blur(6px);
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		white-space: nowrap;
	}
	.refresh-spinner {
		width: 10px;
		height: 10px;
		border-radius: 50%;
		border: 1.5px solid var(--accent-line);
		border-top-color: var(--accent);
		animation: spin 0.8s linear infinite;
		flex-shrink: 0;
	}
	.refresh-label { color: var(--text-secondary); }
	.refresh-pct { color: var(--accent-strong); font-weight: var(--font-weight-semibold); margin-left: 2px; }

	.empty-state {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		color: var(--text-tertiary);
		text-align: center;
		padding: 24px;
	}
	.empty-icon {
		font-size: var(--font-size-4xl);
		opacity: 0.2;
		margin-bottom: 4px;
	}
	.empty-icon.spinning {
		animation: spin 3s linear infinite;
	}
	.empty-title {
		font-weight: var(--font-weight-semibold);
		font-size: var(--font-size-md);
		color: var(--text-secondary);
	}
	.empty-copy {
		font-size: var(--font-size-sm);
		color: var(--text-tertiary);
	}
	.retry-btn {
		margin-top: 12px;
		padding: 8px 20px;
		border-radius: 8px;
		border: 1px solid var(--accent-line);
		background: var(--accent-soft);
		color: var(--accent-strong);
		font-size: var(--font-size-sm);
		cursor: pointer;
		transition: background var(--motion-fast);
	}
	.retry-btn:hover { background: var(--accent-line); }

	@keyframes spin {
		to { transform: rotate(360deg); }
	}
</style>
