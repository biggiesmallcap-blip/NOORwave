<script lang="ts">
	import { tidalSearchTrackToPlayable } from '$lib/utils/track';
	import { lockSeed, branchHere, setRadioRoute, discoverSpaceStore } from './discover_space_store';
	import { REASON_LABELS, REASON_EXPLANATIONS, SOURCE_LABELS, SIDE_PANEL_ACTIONS, ERROR_TOASTS } from './discover_space_story';
	import type { DiscoverTrackNode, DiscoverReason } from './discover_space_types';
	import { api, getApiBase, authFetch, type TidalPlayable } from '$lib/api/client';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { canPlayTrack, getPlayableLabel, type PlayableTrack } from '$lib/player/playable';
	import { showToast } from '$lib/stores/toast';

	interface Props {
		node: DiscoverTrackNode | null;
		seedNode?: DiscoverTrackNode | null;
		onAddToPlaylist?: (node: DiscoverTrackNode) => void;
		onAddToBlend?: (node: DiscoverTrackNode) => void;
	}
	let { node, seedNode = null, onAddToPlaylist, onAddToBlend }: Props = $props();

	let isStartingRadio = $state(false);
	let isHiding = $state(false);
	let resolvingAction = $state<'play-now' | 'play-next' | 'radio' | null>(null);

	// Secondary actions live behind More; it closes on a new node, an outside
	// press, Escape, or once an action runs.
	let moreOpen = $state(false);
	let moreWrap = $state<HTMLDivElement | null>(null);
	$effect(() => {
		void node?.id;
		moreOpen = false;
	});
	function runMore(action: () => unknown): void {
		moreOpen = false;
		void action();
	}

	function nodeFallbackPlayable(targetNode: DiscoverTrackNode): PlayableTrack | null {
		return targetNode.playable;
	}

	function updateNodePlayable(trackId: number, playable: PlayableTrack) {
		if (node?.trackId === trackId) {
			node.playable = playable;
		}
		discoverSpaceStore.update((state) => ({
			...state,
			nodes: state.nodes.map((n) => (n.trackId === trackId ? { ...n, playable } : n)),
		}));
	}

	function pendingToSearchQuery(playable: Extract<PlayableTrack, { kind: 'pending-lastfm' }>): string {
		return [playable.artist, playable.title].filter(Boolean).join(' ');
	}

	// One initial on a tinted tile, like Search, instead of two-letter codes.
	function fallbackText(value: string): string {
		return value.trim().charAt(0).toUpperCase() || 'N';
	}

	async function resolveExternalPlayable(
		targetNode: DiscoverTrackNode,
		playable: PlayableTrack,
	): Promise<PlayableTrack | null> {
		if (playable.kind === 'library' || playable.kind === 'tidal') return playable;
		if (playable.kind === 'unavailable') return null;
		const q = pendingToSearchQuery(playable);
		if (!q) return null;
		const results = await api.searchTidal(q, 1);
		const hit = results.tracks[0];
		if (!hit || hit.tidal_id <= 0) return null;
		const resolved: PlayableTrack = {
			kind: 'tidal',
			tidal_id: hit.tidal_id,
			track: {
				...tidalSearchTrackToPlayable(hit),
				artwork_url: hit.artwork_url ?? targetNode.artworkUrl ?? null,
			},
		};
		updateNodePlayable(targetNode.trackId, resolved);
		return resolved;
	}

	function resolvedTidalTrack(playable: PlayableTrack): TidalPlayable | null {
		if (playable.kind !== 'tidal') return null;
		if (!canPlayTrack(playable)) {
			showToast(getPlayableLabel(playable), 'error');
			return null;
		}
		return playable.track;
	}

	async function handlePlayNow() {
		const targetNode = node;
		if (!targetNode) return;
		resolvingAction = 'play-now';
		try {
			const basePlayable = nodeFallbackPlayable(targetNode);
			if (!basePlayable) return;
			const playable = await resolveExternalPlayable(targetNode, basePlayable);
			if (!playable) {
				showToast(`Couldn't find "${targetNode.title}" on Tidal`, 'error');
				return;
			}
			if (playable.kind === 'library') {
				// Route through the store (optimistic now-playing, intent-seq race
				// guard, error toast + retry, snapshot hydration) instead of a raw
				// api.playTrack that leaves the UI to catch up on its own.
				const { playTrackNow } = await import('$lib/stores/player');
				await playTrackNow(playable.track_id);
				return;
			}
			const tidal = resolvedTidalTrack(playable);
			if (!tidal) {
				return;
			}
			const { playTidalTrackNow } = await import('$lib/stores/player');
			await playTidalTrackNow(tidal);
		} catch {
			showToast('Could not play track', 'error');
		} finally {
			resolvingAction = null;
		}
	}

	async function handlePlayNext() {
		const targetNode = node;
		if (!targetNode) return;
		resolvingAction = 'play-next';
		try {
			const basePlayable = nodeFallbackPlayable(targetNode);
			if (!basePlayable) return;
			const playable = await resolveExternalPlayable(targetNode, basePlayable);
			if (!playable) {
				showToast(`Couldn't find "${targetNode.title}" on Tidal`, 'error');
				return;
			}
			if (playable.kind === 'library') {
				const { playTrackNext } = await import('$lib/stores/player');
				await playTrackNext(playable.track_id);
				return;
			}
			const tidal = resolvedTidalTrack(playable);
			if (!tidal) {
				return;
			}
			const { playTidalTrackNext } = await import('$lib/stores/player');
			await playTidalTrackNext(tidal);
		} catch {
			showToast('Could not queue track', 'error');
		} finally {
			resolvingAction = null;
		}
	}

	async function handleStartRadioHere() {
		const targetNode = node;
		if (!targetNode || isStartingRadio) return;
		isStartingRadio = true;
		resolvingAction = 'radio';
		try {
			const basePlayable = nodeFallbackPlayable(targetNode);
			if (!basePlayable) return;
			const playable = await resolveExternalPlayable(targetNode, basePlayable);
			if (!playable) {
				showToast(`Couldn't find "${targetNode.title}" on Tidal`, 'error');
				return;
			}
			if (playable.kind === 'library') {
				const { startSongRadio } = await import('$lib/stores/player');
				await startSongRadio(playable.track_id);
				return;
			}
			const tidal = resolvedTidalTrack(playable);
			if (!tidal) {
				return;
			}
			const { startTidalSongRadio } = await import('$lib/stores/player');
			await startTidalSongRadio(tidal);
		} catch {
			showToast(ERROR_TOASTS.radioRouteFailed, 'error');
		} finally {
			isStartingRadio = false;
			resolvingAction = null;
		}
	}

	function handleLockAsAnchor() {
		if (!node) return;
		lockSeed(node.trackId);
	}

	function handleBranchHere() {
		if (!node) return;
		branchHere(node);
	}

	function handleAddToPlaylist() {
		if (!node) return;
		onAddToPlaylist?.(node);
	}

	function handleAddToBlend() {
		if (!node) return;
		onAddToBlend?.(node);
	}

	async function handleHideFromRadio() {
		if (!node || isHiding) return;
		isHiding = true;
		try {
			const apiBase = getApiBase();
			const s = $discoverSpaceStore;
			// The feedback contract requires seed + candidate + surface; the old
			// { track_id } body was rejected with 422 and hides never recorded.
			const response = await authFetch(`${apiBase}/api/discovery/feedback`, {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({
					seed_track_id: s.activeSeedId ?? 0,
					candidate_track_id: node.trackId,
					action: 'dismiss',
					surface: 'discover_space',
					session_id: s.sessionId || undefined,
				}),
			});
			if (!response.ok) throw new Error(`Feedback failed: ${response.status}`);
		} catch {
			showToast(ERROR_TOASTS.hideFailed, 'error');
		} finally {
			isHiding = false;
		}
	}
</script>

<svelte:window
	onpointerdown={(event) => {
		if (moreOpen && moreWrap && !moreWrap.contains(event.target as Node)) moreOpen = false;
	}}
	onkeydown={(event) => {
		if (moreOpen && event.key === 'Escape') moreOpen = false;
	}}
/>

<aside
	class="side-panel"
	class:empty={!node && !seedNode}
	role="region"
	aria-label="Track details"
>
	{#if node}
		<div class="panel-header">
			<ArtworkImage
				className="artwork"
				src={node.artworkUrl}
				alt={`${node.title} artwork`}
				size={320}
				fallbackText={fallbackText(node.title)}
				tint={true}
			/>
			<div class="panel-title-group">
				<div class="panel-title" title={node.title}>{node.title}</div>
				<div class="panel-artist">{node.artist}</div>
				{#if node.albumTitle}
					<div class="panel-album">{node.albumTitle}</div>
				{/if}
			</div>
		</div>

		<!-- Sonic fingerprint -->
		{#if node.bpm != null || node.camelotKey || node.energy != null || node.danceability != null}
			<div class="fingerprint" aria-label="Audio features">
				{#if node.bpm != null}<span class="fp-chip"><span class="fp-label">BPM</span>{Math.round(node.bpm)}</span>{/if}
				{#if node.camelotKey}<span class="fp-chip"><span class="fp-label">Key</span>{node.camelotKey}</span>{/if}
				{#if node.energy != null}<span class="fp-chip"><span class="fp-label">Energy</span>{Math.round(node.energy * 100)}%</span>{/if}
				{#if node.danceability != null}<span class="fp-chip"><span class="fp-label">Dance</span>{Math.round(node.danceability * 100)}%</span>{/if}
			</div>
		{/if}

		<!-- Why it connects -->
		<div class="why-section">
			<div class="why-head">
				<span class="reason-pill">{REASON_LABELS[node.primaryReason]}</span>
				<span class="conf-label">{Math.round(node.confidence * 100)}% match</span>
			</div>
			<p class="why-line">{node.why || REASON_EXPLANATIONS[node.primaryReason]}</p>
			<div class="meta-row">
				{#if node.topGenre}<span class="meta-tag genre">{node.topGenre}</span>{/if}
				<span class="meta-tag">{SOURCE_LABELS[node.source]}</span>
				{#each node.reasonTags.slice(1) as reason}
					<span class="meta-tag">{REASON_LABELS[reason]}</span>
				{/each}
				{#if node.isColdStart}<span class="meta-tag">Cold start</span>{/if}
			</div>
		</div>

		<!-- Actions: three primary, the rest behind More -->
		<div class="actions" role="group" aria-label="Track actions">
			<button
				class="action-btn primary"
				onclick={handlePlayNow}
				disabled={resolvingAction !== null}
				aria-busy={resolvingAction === 'play-now'}
				aria-label="Play {node.title} now"
			>
				{#if resolvingAction === 'play-now'}<span class="button-spinner" aria-hidden="true"></span>{/if}
				Play
			</button>
			<button
				class="action-btn"
				onclick={handleBranchHere}
				disabled={node.trackId === $discoverSpaceStore.activeSeedId}
				aria-label="Branch into {node.title}"
			>
				{SIDE_PANEL_ACTIONS.branchHere}
			</button>
			<button
				class="action-btn"
				onclick={handleStartRadioHere}
				disabled={isStartingRadio || resolvingAction !== null}
				aria-busy={resolvingAction === 'radio'}
				aria-label="Start radio from {node.title}"
			>
				{#if resolvingAction === 'radio' || isStartingRadio}<span class="button-spinner" aria-hidden="true"></span>{/if}
				Radio
			</button>
			<div class="more-wrap" bind:this={moreWrap}>
				<button
					class="action-btn icon"
					onclick={() => (moreOpen = !moreOpen)}
					aria-haspopup="menu"
					aria-expanded={moreOpen}
					aria-label="More actions for {node.title}"
				>
					<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="3.5" cy="8" r="1.3" /><circle cx="8" cy="8" r="1.3" /><circle cx="12.5" cy="8" r="1.3" /></svg>
				</button>
				{#if moreOpen}
					<div class="more-menu" role="menu">
						<button role="menuitem" onclick={() => runMore(handlePlayNext)} disabled={resolvingAction !== null}>
							{resolvingAction === 'play-next' ? 'Resolving...' : SIDE_PANEL_ACTIONS.playNext}
						</button>
						<button role="menuitem" onclick={() => runMore(handleLockAsAnchor)}>
							{$discoverSpaceStore.lockedSeedId === node.trackId ? 'Locked as anchor' : SIDE_PANEL_ACTIONS.lockAsAnchor}
						</button>
						<button role="menuitem" onclick={() => runMore(handleAddToPlaylist)}>
							{node.inPlaylistBuilder ? 'Remove from playlist' : SIDE_PANEL_ACTIONS.addToPlaylist}
						</button>
						<button role="menuitem" onclick={() => runMore(handleAddToBlend)} disabled={$discoverSpaceStore.blendSeeds.length >= 4}>
							Add to blend
						</button>
						<button role="menuitem" class="destructive" onclick={() => runMore(handleHideFromRadio)} disabled={isHiding}>
							{isHiding ? 'Hiding...' : SIDE_PANEL_ACTIONS.hideFromRadio}
						</button>
					</div>
				{/if}
			</div>
		</div>

		<!-- Screen reader summary -->
		<p class="sr-only">
			{node.title} by {node.artist}.
			{REASON_LABELS[node.primaryReason]}: {REASON_EXPLANATIONS[node.primaryReason]}
			Confidence: {Math.round(node.confidence * 100)}%.
			Source: {SOURCE_LABELS[node.source]}.
		</p>
	{:else if seedNode}
		<!-- Idle: the anchor the map grows from -->
		<div class="panel-idle">
			<ArtworkImage
				className="idle-artwork"
				src={seedNode.artworkUrl}
				size={320}
				fallbackText={fallbackText(seedNode.title)}
				decorative={true}
				tint={true}
			/>
			<div class="idle-anchor-info">
				<div class="idle-anchor-label">Anchor</div>
				<div class="idle-anchor-title" title={seedNode.title}>{seedNode.title}</div>
				<div class="idle-anchor-artist">{seedNode.artist}</div>
				{#if seedNode.genres.length > 0}
					<div class="idle-genres">
						{#each seedNode.genres.slice(0, 3) as g}
							<span class="meta-tag genre">{g}</span>
						{/each}
					</div>
				{/if}
			</div>
		</div>
	{/if}
</aside>

<style>
	/* No inner scroll: the More menu has to escape the card, and the results
	   list under it is the part that gives up height. Stacks above the list so
	   the menu paints over it. */
	.side-panel {
		position: relative;
		z-index: 2;
		flex-shrink: 0;
		padding: var(--space-4);
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		border-radius: var(--radius-lg);
		/* Frosted, in the theme colours, like the results list under it. */
		background: color-mix(in srgb, var(--bg-elevated) 74%, transparent);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		border: 1px solid var(--panel-border);
	}
	.side-panel.empty { display: none; }

	.panel-header { display: flex; gap: var(--space-3); align-items: center; min-width: 0; }
	.panel-header :global(.artwork) { width: 72px; height: 72px; border-radius: var(--radius-md); flex-shrink: 0; box-shadow: 0 4px 14px var(--player-art-shadow); }
	.panel-header :global(img.artwork) { object-fit: cover; display: block; }
	.panel-header :global(.artwork.fallback) { display: grid; place-items: center; }
	.panel-header :global(.artwork.fallback span) { font-size: var(--font-size-md); font-weight: var(--font-weight-semibold); }
	.panel-title-group { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
	.panel-title {
		font-weight: var(--font-weight-semibold);
		font-size: var(--font-size-md);
		line-height: var(--line-height-snug);
		color: var(--text-primary);
		display: -webkit-box;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
	.panel-artist { font-size: var(--font-size-sm); color: var(--text-secondary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.panel-album { font-size: var(--font-size-xs); color: var(--text-tertiary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

	.fingerprint { display: flex; flex-wrap: wrap; gap: 6px; }
	.fp-chip {
		display: inline-flex;
		align-items: baseline;
		gap: 5px;
		padding: 4px 9px;
		border-radius: 999px;
		background: var(--bg-surface);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
	}
	.fp-label { font-size: var(--font-size-2xs); font-weight: var(--font-weight-medium); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-tertiary); }

	.why-section { display: flex; flex-direction: column; gap: 6px; }
	.why-head { display: flex; align-items: center; justify-content: space-between; gap: var(--space-2); }
	.reason-pill { font-size: var(--font-size-xs); font-weight: var(--font-weight-semibold); color: var(--accent-strong); }
	.conf-label { font-size: var(--font-size-xs); color: var(--text-tertiary); white-space: nowrap; font-variant-numeric: tabular-nums; }
	.why-line { margin: 0; font-size: var(--font-size-sm); line-height: var(--line-height-normal); color: var(--text-secondary); }

	.meta-row, .idle-genres { display: flex; flex-wrap: wrap; gap: 4px; }
	.meta-tag {
		padding: 2px 8px;
		border-radius: 999px;
		background: var(--bg-surface);
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
	}
	.meta-tag.genre { background: color-mix(in srgb, var(--state-success) 14%, transparent); color: var(--state-success); }

	.actions { display: flex; gap: 6px; align-items: center; }
	.action-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		min-height: 34px;
		padding: 0 var(--space-3);
		border-radius: 999px;
		border: 1px solid var(--border-muted);
		background: transparent;
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		white-space: nowrap;
		cursor: pointer;
		transition: background var(--motion-fast), border-color var(--motion-fast), filter var(--motion-fast);
	}
	.action-btn:hover:not(:disabled) { background: var(--bg-hover); border-color: var(--border-strong); }
	.action-btn.primary { flex: 1; border-color: var(--accent); background: var(--accent); color: var(--text-on-accent); }
	.action-btn.primary:hover:not(:disabled) { background: var(--accent); filter: brightness(1.1); }
	.action-btn.icon { width: 34px; padding: 0; }
	.action-btn.icon svg { width: 16px; height: 16px; fill: currentColor; }
	.action-btn:disabled { opacity: 0.4; cursor: not-allowed; }

	.more-wrap { position: relative; margin-left: auto; }
	.more-menu {
		position: absolute;
		right: 0;
		top: calc(100% + 6px);
		z-index: 20;
		min-width: 190px;
		padding: 4px;
		display: flex;
		flex-direction: column;
		border-radius: var(--radius-md);
		background: var(--bg-surface-strong);
		border: 1px solid var(--panel-border);
		box-shadow: var(--panel-shadow);
	}
	.more-menu button {
		min-height: 34px;
		padding: 0 var(--space-3);
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		text-align: left;
		cursor: pointer;
	}
	.more-menu button:hover:not(:disabled), .more-menu button:focus-visible { background: var(--bg-hover); }
	.more-menu button.destructive { color: var(--state-error); }
	.more-menu button:disabled { opacity: 0.4; cursor: not-allowed; }

	.button-spinner {
		display: inline-block;
		width: 0.75rem;
		height: 0.75rem;
		border: 2px solid var(--border-strong);
		border-top-color: currentColor;
		border-radius: 999px;
		animation: button-spin 0.8s linear infinite;
	}
	@keyframes button-spin {
		to { transform: rotate(360deg); }
	}

	.sr-only {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		border: 0;
	}

	/* Idle: the anchor the map grows from */
	.panel-idle { display: flex; gap: var(--space-3); align-items: center; min-width: 0; }
	.panel-idle :global(.idle-artwork) { width: 56px; height: 56px; border-radius: var(--radius-md); flex-shrink: 0; box-shadow: 0 0 0 2px var(--accent-line); }
	.panel-idle :global(img.idle-artwork) { object-fit: cover; display: block; }
	.panel-idle :global(.idle-artwork.fallback) { display: grid; place-items: center; }
	.panel-idle :global(.idle-artwork.fallback span) { font-size: var(--font-size-xs); font-weight: var(--font-weight-semibold); }
	.idle-anchor-info { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
	.idle-anchor-label {
		font-size: var(--font-size-2xs); text-transform: uppercase; letter-spacing: 0.1em;
		color: var(--accent-strong);
	}
	.idle-anchor-title { font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.idle-anchor-artist { font-size: var(--font-size-xs); color: var(--text-secondary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.idle-genres { margin-top: 4px; }

</style>
