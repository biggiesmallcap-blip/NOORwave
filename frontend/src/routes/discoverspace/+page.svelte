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
	let seedLocked = $derived($discoverSpaceStore.lockedSeedId !== null);
	let seedTitle = $derived(
		anchorNode?.title
		?? ($currentTrack && $currentTrack.id === $discoverSpaceStore.activeSeedId ? $currentTrack.title : null)
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
	<!-- Header: title, seed, search and help on one row -->
	<div class="page-header">
		<div class="header-text">
			<h1 class="t-page-title">{PAGE_TITLE}</h1>
			<p class="tagline">{PAGE_SUBTITLE}</p>
		</div>

		{#if $discoverSpaceStore.activeSeedId !== null}
			<div class="seed-chip" class:locked={seedLocked}>
				<span class="seed-source">{seedLocked ? 'Locked seed' : 'Seeded from playing'}</span>
				{#if seedTitle}
					<span class="seed-title" title={seedTitle}>{seedTitle}</span>
				{/if}
				<button
					class="seed-toggle"
					onclick={handleToggleLock}
					disabled={$currentTrack?.id == null && $discoverSpaceStore.lockedSeedId === null}
					aria-pressed={seedLocked}
					title={seedLocked ? 'Follow what is playing again' : 'Keep this seed while the music changes'}
				>
					<svg viewBox="0 0 16 16" aria-hidden="true">
						{#if seedLocked}
							<path d="M4.5 7V5a3.5 3.5 0 0 1 7 0v2" />
						{:else}
							<path d="M4.5 7V5a3.5 3.5 0 0 1 6.8-1.2" />
						{/if}
						<rect x="3" y="7" width="10" height="7" rx="1.5" />
					</svg>
					{seedLocked ? 'Unlock' : 'Lock'}
				</button>
			</div>
		{/if}

		<form class="search-form" onsubmit={handleSearch}>
			<SearchField
				bind:value={searchQuery}
				variant="page"
				size="sm"
				fill
				disabled={isSearching}
				placeholder={SEARCH_PLACEHOLDER}
			/>
			<button
				class="search-btn"
				type="submit"
				disabled={isSearching || !searchQuery.trim()}
				aria-busy={isSearching}
				aria-label="Jump to this sound"
			>
				<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 8h9M8.5 4.5 12 8l-3.5 3.5" /></svg>
			</button>
		</form>
		<DiscoverHelp />
	</div>

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

	<!-- Stage and rail: the rail drops under the stage when the workspace is narrow -->
	<div class="page-layout">
		<div class="canvas-area" data-theme="dark" bind:this={stageEl}>
			{#if resolvedSeedId === null}
				<div class="empty-state">
					<div class="empty-title">{EMPTY_STATE.noSeed.title}</div>
					<div class="empty-copy">{EMPTY_STATE.noSeed.copy}</div>
				</div>
			{:else if $discoverSpaceStore.loading && $discoverSpaceStore.nodes.length === 0}
				<div class="empty-state">
					<span class="empty-spinner" aria-hidden="true"></span>
					<div class="empty-title">{EMPTY_STATE.loading.title}</div>
					<div class="empty-copy">{EMPTY_STATE.loading.copy}</div>
				</div>
			{:else if $discoverSpaceStore.error && $discoverSpaceStore.nodes.length === 0}
				<div class="empty-state">
					<div class="empty-title">{EMPTY_STATE.loadFailed.title}</div>
					<div class="empty-copy">{EMPTY_STATE.loadFailed.copy}</div>
					<button
						class="retry-btn"
						onclick={() => resolvedSeedId && loadSpace($discoverSpaceStore.mode, resolvedSeedId, undefined, resolvedSeedSource, $currentTrack?.id ?? null)}
					>Retry</button>
				</div>
			{:else if $discoverSpaceStore.nodes.length === 0}
				<div class="empty-state">
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

			<!-- Toolbar floats over the stage: lens on the left, filters on the right -->
			<div class="canvas-toolbar">
				{#if $discoverSpaceStore.nodes.length > 0}
					<DiscoverLensControl />
				{/if}
				<div class="toolbar-end">
					<DiscoverFilterBar />
				</div>
			</div>

			{#if $discoverSpaceStore.refreshProgress !== null}
				{@const rp = $discoverSpaceStore.refreshProgress}
				<div class="canvas-overlay bottom-left">
					<div class="refresh-pill">
						<span class="refresh-spinner" aria-hidden="true"></span>
						<span class="refresh-label">
							{#if rp.stage === 'loading'}Loading embeddings{:else if rp.stage === 'computing'}Computing similarity{:else if rp.stage === 'saving'}Saving connections{:else}{rp.stage}{/if}
						</span>
						<span class="refresh-pct">{Math.round(rp.progress * 100)}%</span>
					</div>
				</div>
			{:else if $discoverSpaceStore.nodes.length > 0 && !selectedNode}
				<p class="canvas-hint">Hover a star to inspect it. Click one to open it.</p>
			{/if}

			<div class="canvas-overlay bottom-center">
				<DiscoverTrainingStrip />
			</div>
		</div>

		<div class="space-rail">
			<DiscoverSidePanel
				node={selectedNode}
				seedNode={anchorNode}
				onAddToPlaylist={handleAddToPlaylist}
				onAddToBlend={handleAddToBlend}
			/>
			<DiscoverRankedList onSelectNode={(node) => handleSelectNode(node)} />
		</div>
	</div>

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
		gap: var(--space-3);
		overflow: hidden;
	}

	/* -- Header ----------------------------------------------------------- */
	.page-header {
		display: flex;
		align-items: center;
		gap: var(--space-4);
		flex-shrink: 0;
		min-width: 0;
	}
	.header-text { display: flex; flex-direction: column; gap: 2px; flex-shrink: 0; }
	.tagline {
		margin: 0;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
	}
	h1 {
		margin: 0;
		color: var(--text-primary);
	}

	.seed-chip {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		min-width: 0;
		flex: 0 1 auto;
		max-width: 460px;
		padding: 4px 4px 4px var(--space-3);
		border-radius: 999px;
		background: var(--bg-surface);
		font-size: var(--font-size-xs);
	}
	.seed-chip.locked { background: var(--accent-soft); }
	.seed-source { color: var(--text-tertiary); white-space: nowrap; }
	.seed-title {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
	}
	.seed-toggle {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		flex-shrink: 0;
		min-height: 26px;
		padding: 0 10px;
		border: 1px solid var(--border-muted);
		border-radius: 999px;
		background: transparent;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		cursor: pointer;
		transition: background var(--motion-fast), color var(--motion-fast), border-color var(--motion-fast);
	}
	.seed-toggle svg { width: 12px; height: 12px; fill: none; stroke: currentColor; stroke-width: 1.5; stroke-linecap: round; }
	.seed-toggle:hover:not(:disabled) { color: var(--text-primary); border-color: var(--border-strong); }
	.seed-chip.locked .seed-toggle { color: var(--accent-strong); border-color: var(--accent-line); }
	.seed-toggle:disabled { opacity: 0.4; cursor: not-allowed; }

	.search-form {
		display: flex;
		align-items: center;
		gap: 6px;
		flex: 1 0 260px;
		min-width: 0;
		max-width: 420px;
		margin-left: auto;
	}
	.search-btn {
		display: inline-grid;
		place-items: center;
		flex-shrink: 0;
		width: 36px;
		height: 36px;
		border-radius: var(--radius-md);
		border: 0;
		background: var(--accent);
		color: var(--text-on-accent);
		cursor: pointer;
		transition: filter var(--motion-fast), opacity var(--motion-fast);
	}
	.search-btn svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; }
	.search-btn:hover:not(:disabled) { filter: brightness(1.1); }
	.search-btn:disabled { opacity: 0.35; cursor: default; }

	/* -- Blend and automix strips ----------------------------------------- */
	.automix-bar {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 6px 14px;
		border-radius: 999px;
		background: var(--accent-soft);
		flex-shrink: 0;
		width: fit-content;
		max-width: 100%;
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
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
		flex-wrap: wrap;
		gap: 10px;
		padding: 8px 10px;
		border-radius: var(--radius-md);
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
		border-radius: 999px;
		border: 1px solid var(--border-muted);
		background: transparent;
		color: var(--text-secondary);
		cursor: pointer;
	}
	.blend-action.primary {
		border-color: var(--accent);
		background: var(--accent);
		color: var(--text-on-accent);
	}
	.blend-action.subtle { border-color: transparent; color: var(--text-tertiary); }
	.blend-action:hover:not(:disabled) { background: var(--bg-hover); color: var(--text-primary); }
	.blend-action.primary:hover:not(:disabled) { background: var(--accent); color: var(--text-on-accent); filter: brightness(1.1); }
	.blend-action:disabled {
		opacity: 0.42;
		cursor: not-allowed;
	}

	/* -- Stage and rail --------------------------------------------------- */
	.page-layout {
		flex: 1;
		display: grid;
		grid-template-columns: minmax(0, 1fr) clamp(300px, 28%, 360px);
		gap: var(--space-3);
		min-height: 0;
	}

	.canvas-area {
		position: relative;
		min-height: 0;
		height: 100%;
		border-radius: var(--radius-lg);
		overflow: hidden;
		background: var(--bg-base);
		color: var(--text-primary);
		border: 1px solid var(--border-subtle);
	}

	.space-rail {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		min-height: 0;
		min-width: 0;
	}

	.canvas-toolbar {
		position: absolute;
		top: 12px;
		left: 12px;
		right: 12px;
		z-index: 10;
		display: flex;
		flex-wrap: wrap;
		align-items: flex-start;
		gap: var(--space-2);
		pointer-events: none;
	}
	.canvas-toolbar > :global(*) { pointer-events: auto; }
	.toolbar-end { margin-left: auto; }

	.canvas-overlay {
		position: absolute;
		z-index: 10;
		pointer-events: auto;
	}
	.canvas-overlay.bottom-left { bottom: 12px; left: 12px; }
	.canvas-overlay.bottom-center {
		bottom: 12px;
		left: 50%;
		transform: translateX(-50%);
	}

	.canvas-hint {
		position: absolute;
		left: 14px;
		bottom: 14px;
		z-index: 5;
		margin: 0;
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
		pointer-events: none;
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
		gap: 6px;
		text-align: center;
		padding: 24px;
	}
	.empty-spinner {
		width: 22px;
		height: 22px;
		margin-bottom: 6px;
		border-radius: 50%;
		border: 2px solid var(--border-strong);
		border-top-color: var(--accent);
		animation: spin 0.9s linear infinite;
	}
	.empty-title {
		font-weight: var(--font-weight-semibold);
		font-size: var(--font-size-md);
		color: var(--text-primary);
	}
	.empty-copy {
		max-width: 36ch;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
	}
	.retry-btn {
		margin-top: 12px;
		padding: 8px 20px;
		border-radius: 999px;
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

	/* Narrow workspace: stack the rail under a fixed-height stage and let the
	   page scroll, instead of squeezing the stage to a sliver. */
	@container workspace (max-width: 860px) {
		.discoverspace-page { height: auto; overflow: visible; }
		.page-header { flex-wrap: wrap; }
		/* Basis 0 so the chip shrinks into the first row instead of pushing help onto its own line. */
		.seed-chip { flex: 1 1 0; max-width: max-content; }
		.search-form { flex-basis: 100%; max-width: none; margin-left: 0; order: 3; }
		.page-layout { flex: none; grid-template-columns: minmax(0, 1fr); }
		.canvas-area { height: clamp(360px, 62vh, 620px); }
		.space-rail {
			display: grid;
			grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
			align-items: start;
		}
	}
</style>
