<script lang="ts">
	import { tidalSearchTrackToPlayable } from '$lib/utils/track';
	import { lockSeed, branchHere, setRadioRoute, discoverSpaceStore } from './discover_space_store';
	import { REASON_LABELS, REASON_EXPLANATIONS, SOURCE_LABELS, SIDE_PANEL_ACTIONS, ERROR_TOASTS, LENS_LABELS, LENS_DESCRIPTIONS } from './discover_space_story';
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

	function fallbackText(value: string): string {
		return value.trim().slice(0, 2).toUpperCase() || 'NOOR';
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

<aside
	class="side-panel"
	class:empty={!node}
	role="region"
	aria-label="Track details"
>
	{#if node}
		<div class="panel-header">
			{#if node.artworkUrl}
				<ArtworkImage
					className="artwork"
					src={node.artworkUrl}
					alt={`${node.title} artwork`}
					size={320}
					fallbackText={fallbackText(node.title)}
				/>
			{:else}
				<div class="artwork-placeholder" aria-hidden="true"></div>
			{/if}
			<div class="panel-title-group">
				<div class="panel-title">{node.title}</div>
				<div class="panel-artist">{node.artist}</div>
				{#if node.albumTitle}
					<div class="panel-album">{node.albumTitle}</div>
				{/if}
			</div>
		</div>

		<!-- Sonic fingerprint chips -->
		<div class="fingerprint" aria-label="Audio features">
			{#if node.energy != null}
				<div class="fp-chip">
					<span class="fp-label">Energy</span>
					<span class="fp-value">{Math.round(node.energy * 100)}%</span>
					<div class="fp-bar"><div class="fp-fill" style:width="{node.energy * 100}%"></div></div>
				</div>
			{/if}
			{#if node.danceability != null}
				<div class="fp-chip">
					<span class="fp-label">Dance</span>
					<span class="fp-value">{Math.round(node.danceability * 100)}%</span>
					<div class="fp-bar"><div class="fp-fill" style:width="{node.danceability * 100}%"></div></div>
				</div>
			{/if}
			{#if node.bpm != null}
				<div class="fp-chip narrow">
					<span class="fp-label">BPM</span>
					<span class="fp-value">{Math.round(node.bpm)}</span>
				</div>
			{/if}
			{#if node.camelotKey}
				<div class="fp-chip narrow">
					<span class="fp-label">Key</span>
					<span class="fp-value">{node.camelotKey}</span>
				</div>
			{/if}
		</div>

		<!-- Why it connects -->
		<div class="why-section">
			<div class="why-heading">Why it connects</div>
			{#if node.why}
				<div class="why-line">{node.why}</div>
			{/if}
			<div class="why-reason">
				<span class="reason-pill">{REASON_LABELS[node.primaryReason]}</span>
				<span class="reason-explanation">{REASON_EXPLANATIONS[node.primaryReason]}</span>
			</div>
			{#if node.reasonTags.length > 1}
				<div class="extra-reasons">
					{#each node.reasonTags.slice(1) as reason}
						<span class="reason-mini">{REASON_LABELS[reason]}</span>
					{/each}
				</div>
			{/if}
			<div class="conf-row">
				<div class="conf-bar"><div class="conf-fill" style:width="{node.confidence * 100}%"></div></div>
				<span class="conf-label">{Math.round(node.confidence * 100)}% confidence</span>
			</div>
		</div>

		<!-- Genre / source metadata -->
		<div class="meta-row">
			{#if node.topGenre}<span class="meta-tag genre">{node.topGenre}</span>{/if}
			<span class="meta-tag source">{SOURCE_LABELS[node.source]}</span>
			{#if node.isColdStart}<span class="meta-tag cold">Cold start</span>{/if}
		</div>

		<!-- Action stack -->
		<div class="actions" role="group" aria-label="Track actions">
			<button
				class="action-btn primary"
				onclick={handlePlayNow}
				disabled={resolvingAction !== null}
				aria-busy={resolvingAction === 'play-now'}
				aria-label="Play {node.title} now"
			>
				{#if resolvingAction === 'play-now'}<span class="button-spinner" aria-hidden="true"></span> Resolving...{:else}{SIDE_PANEL_ACTIONS.playNow}{/if}
			</button>
			<button
				class="action-btn"
				onclick={handlePlayNext}
				disabled={resolvingAction !== null}
				aria-busy={resolvingAction === 'play-next'}
				aria-label="Play {node.title} next"
			>
				{#if resolvingAction === 'play-next'}<span class="button-spinner" aria-hidden="true"></span> Resolving...{:else}{SIDE_PANEL_ACTIONS.playNext}{/if}
			</button>
			<button
				class="action-btn"
				onclick={handleStartRadioHere}
				disabled={isStartingRadio || resolvingAction !== null}
				aria-busy={resolvingAction === 'radio'}
				aria-label="Start radio from {node.title}"
			>
				{#if resolvingAction === 'radio'}<span class="button-spinner" aria-hidden="true"></span> Resolving...{:else if isStartingRadio}Starting...{:else}{SIDE_PANEL_ACTIONS.startRadioHere}{/if}
			</button>
			<button
				class="action-btn"
				onclick={handleBranchHere}
				disabled={node.trackId === $discoverSpaceStore.activeSeedId}
				aria-label="Branch into {node.title}"
			>
				{SIDE_PANEL_ACTIONS.branchHere}
			</button>
			<button class="action-btn" onclick={handleLockAsAnchor} aria-label="Lock {node.title} as seed anchor">
				{$discoverSpaceStore.lockedSeedId === node.trackId ? '🔒 Locked' : SIDE_PANEL_ACTIONS.lockAsAnchor}
			</button>
			<button
				class="action-btn"
				class:active={node.inPlaylistBuilder}
				onclick={handleAddToPlaylist}
				aria-label="{node.inPlaylistBuilder ? 'Remove from' : 'Add to'} playlist"
			>
				{node.inPlaylistBuilder ? '★ In playlist' : SIDE_PANEL_ACTIONS.addToPlaylist}
			</button>
			<button
				class="action-btn"
				onclick={handleAddToBlend}
				disabled={$discoverSpaceStore.blendSeeds.length >= 4}
				aria-label="Add {node.title} to blend"
			>
				Add to blend
			</button>
			<button
				class="action-btn destructive"
				onclick={handleHideFromRadio}
				disabled={isHiding}
				aria-label="Hide {node.title} from radio"
			>
				{isHiding ? 'Hiding…' : SIDE_PANEL_ACTIONS.hideFromRadio}
			</button>
		</div>

		<!-- Screen reader summary -->
		<p class="sr-only">
			{node.title} by {node.artist}.
			{REASON_LABELS[node.primaryReason]}: {REASON_EXPLANATIONS[node.primaryReason]}
			Confidence: {Math.round(node.confidence * 100)}%.
			Source: {SOURCE_LABELS[node.source]}.
		</p>
	{:else if seedNode}
		<!-- Idle state: show anchor star context + instructions -->
		<div class="panel-idle">
			<div class="idle-anchor">
				{#if seedNode.artworkUrl}
					<ArtworkImage
						className="idle-artwork"
						src={seedNode.artworkUrl}
						size={320}
						fallbackText={fallbackText(seedNode.title)}
						decorative={true}
					/>
				{:else}
					<div class="idle-artwork-placeholder" aria-hidden="true"></div>
				{/if}
				<div class="idle-anchor-info">
					<div class="idle-anchor-label">Anchor Star</div>
					<div class="idle-anchor-title">{seedNode.title}</div>
					<div class="idle-anchor-artist">{seedNode.artist}</div>
				</div>
			</div>

			{#if seedNode.genres.length > 0}
				<div class="idle-genres">
					{#each seedNode.genres.slice(0, 3) as g}
						<span class="idle-genre-tag">{g}</span>
					{/each}
				</div>
			{/if}

			<div class="idle-lens">
				<span class="idle-lens-name">{LENS_LABELS[$discoverSpaceStore.lens]}</span>
				<span class="idle-lens-desc">{LENS_DESCRIPTIONS[$discoverSpaceStore.lens]}</span>
			</div>

			<div class="idle-instructions">
				<div class="idle-instr-row"><span class="idle-dot" aria-hidden="true">·</span> Hover a star to inspect it</div>
				<div class="idle-instr-row"><span class="idle-dot" aria-hidden="true">·</span> Click a star to open its chart</div>
				<div class="idle-instr-row"><span class="idle-dot" aria-hidden="true">·</span> Start radio here to trace a route</div>
			</div>
		</div>
	{:else}
		<div class="panel-empty">
			<span class="panel-empty-icon" aria-hidden="true">◈</span>
			<span>Play something to seed the map</span>
		</div>
	{/if}
</aside>

<style>
	.side-panel {
		height: 100%;
		overflow-y: auto;
		padding: 12px;
		display: flex;
		flex-direction: column;
		gap: 12px;
		border-left: 1px solid var(--border-subtle);
	}
	.panel-empty {
		height: 100%;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}
	.panel-empty-icon { font-size: var(--font-size-2xl); opacity: 0.3; }

	.panel-header { display: flex; gap: 10px; align-items: flex-start; }
	.panel-header :global(.artwork) { width: 56px; height: 56px; border-radius: 6px; flex-shrink: 0; }
	.panel-header :global(img.artwork) { object-fit: cover; display: block; }
	.panel-header :global(.artwork.fallback) { display: grid; place-items: center; background: var(--bg-surface); }
	.panel-header :global(.artwork.fallback span) { font-size: var(--font-size-xs); font-weight: var(--font-weight-semibold); color: var(--text-secondary); }
	.artwork-placeholder { width: 56px; height: 56px; border-radius: 6px; background: var(--bg-surface); flex-shrink: 0; }
	.panel-title-group { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
	.panel-title { font-weight: var(--font-weight-semibold); font-size: var(--font-size-sm); color: var(--text-primary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.panel-artist { font-size: var(--font-size-xs); color: var(--text-secondary); }
	.panel-album { font-size: var(--font-size-xs); color: var(--text-tertiary); }

	.fingerprint { display: flex; flex-wrap: wrap; gap: 6px; }
	.fp-chip {
		background: var(--bg-surface);
		border: 1px solid var(--panel-border);
		border-radius: 6px;
		padding: 5px 8px;
		display: flex;
		flex-direction: column;
		gap: 3px;
		flex: 1;
		min-width: 60px;
	}
	.fp-chip.narrow { min-width: 44px; flex: 0; }
	.fp-label { font-size: var(--font-size-2xs); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-tertiary); }
	.fp-value { font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); font-variant-numeric: tabular-nums; }
	.fp-bar { height: 3px; background: var(--bg-hover); border-radius: 999px; overflow: hidden; }
	.fp-fill { height: 100%; background: var(--accent); border-radius: 999px; }

	.why-section { display: flex; flex-direction: column; gap: 6px; }
	.why-line { font-size: var(--font-size-xs); color: var(--accent-strong); }
	.why-heading { font-size: var(--font-size-2xs); text-transform: uppercase; letter-spacing: 0.1em; color: var(--text-tertiary); }
	.why-reason { display: flex; flex-direction: column; gap: 3px; }
	.reason-pill { font-size: var(--font-size-2xs); font-weight: var(--font-weight-semibold); color: var(--accent-strong); }
	.reason-explanation { font-size: var(--font-size-xs); color: var(--text-tertiary); line-height: var(--line-height-normal); }
	.extra-reasons { display: flex; flex-wrap: wrap; gap: 4px; }
	.reason-mini {
		padding: 1px 6px;
		border-radius: 4px;
		background: var(--accent-soft);
		color: var(--accent-strong);
		font-size: var(--font-size-2xs);
	}
	.conf-row { display: flex; align-items: center; gap: 6px; }
	.conf-bar { flex: 1; height: 3px; background: var(--bg-hover); border-radius: 999px; overflow: hidden; }
	.conf-fill { height: 100%; background: var(--accent); border-radius: 999px; }
	.conf-label { font-size: var(--font-size-2xs); color: var(--text-tertiary); white-space: nowrap; }

	.meta-row { display: flex; flex-wrap: wrap; gap: 4px; }
	.meta-tag { padding: 2px 7px; border-radius: 4px; font-size: var(--font-size-2xs); }
	.meta-tag.genre { background: color-mix(in srgb, var(--state-success) 14%, transparent); color: var(--state-success); }
	.meta-tag.source { background: var(--accent-soft); color: var(--accent-strong); }
	.meta-tag.cold { background: var(--bg-surface); color: var(--text-tertiary); }

	.actions { display: flex; flex-direction: column; gap: 5px; }
	.action-btn {
		padding: 8px 12px;
		border-radius: 8px;
		border: 1px solid var(--panel-border);
		background: var(--bg-surface);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		cursor: pointer;
		text-align: left;
		transition: background var(--motion-fast), color var(--motion-fast);
	}
	.action-btn:hover:not(:disabled) { background: var(--bg-hover); color: var(--text-primary); }
	.action-btn.primary {
		background: var(--accent-line);
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}
	.action-btn.primary:hover:not(:disabled) { background: var(--accent-line); }
	.action-btn.active { color: var(--state-warning); border-color: color-mix(in srgb, var(--state-warning) 40%, transparent); }
	.action-btn.destructive { color: var(--state-error); }
	.action-btn.destructive:hover:not(:disabled) { color: var(--state-error); background: color-mix(in srgb, var(--state-error) 10%, transparent); }
	.action-btn:disabled { opacity: 0.4; cursor: not-allowed; }
	.button-spinner {
		display: inline-block;
		width: 0.75rem;
		height: 0.75rem;
		margin-right: 6px;
		border: 2px solid var(--border-strong);
		border-top-color: currentColor;
		border-radius: 999px;
		vertical-align: -1px;
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

	/* Idle state (seed selected but no node clicked) */
	.panel-idle {
		display: flex;
		flex-direction: column;
		gap: 14px;
		padding: 4px 0;
	}
	.idle-anchor {
		display: flex;
		gap: 10px;
		align-items: flex-start;
	}
	.idle-anchor :global(.idle-artwork) {
		width: 40px; height: 40px; border-radius: 5px; flex-shrink: 0;
		box-shadow: 0 0 0 1px var(--accent-glow), 0 0 10px var(--accent-glow);
	}
	.idle-anchor :global(img.idle-artwork) { object-fit: cover; display: block; }
	.idle-anchor :global(.idle-artwork.fallback) { display: grid; place-items: center; background: var(--accent-soft); border: 1px solid var(--accent-line); }
	.idle-anchor :global(.idle-artwork.fallback span) { font-size: var(--font-size-2xs); font-weight: var(--font-weight-semibold); color: var(--text-secondary); }
	.idle-artwork-placeholder {
		width: 40px; height: 40px; border-radius: 5px; flex-shrink: 0;
		background: var(--accent-soft);
		border: 1px solid var(--accent-line);
	}
	.idle-anchor-info { flex: 1; min-width: 0; }
	.idle-anchor-label {
		font-size: var(--font-size-2xs); text-transform: uppercase; letter-spacing: 0.1em;
		color: var(--accent-strong); margin-bottom: 2px;
	}
	.idle-anchor-title { font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); color: var(--text-primary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.idle-anchor-artist { font-size: var(--font-size-xs); color: var(--text-tertiary); }

	.idle-genres { display: flex; flex-wrap: wrap; gap: 4px; }
	.idle-genre-tag {
		padding: 2px 7px; border-radius: 4px; font-size: var(--font-size-2xs);
		background: color-mix(in srgb, var(--state-success) 14%, transparent); color: var(--state-success);
	}

	.idle-lens {
		display: flex; flex-direction: column; gap: 3px;
		padding: 8px 10px;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		border-radius: 6px;
	}
	.idle-lens-name { font-size: var(--font-size-2xs); text-transform: uppercase; letter-spacing: 0.08em; color: var(--accent-strong); }
	.idle-lens-desc { font-size: var(--font-size-xs); color: var(--text-tertiary); line-height: var(--line-height-normal); }

	.idle-instructions { display: flex; flex-direction: column; gap: 5px; padding-top: 2px; }
	.idle-instr-row { font-size: var(--font-size-xs); color: var(--text-tertiary); display: flex; align-items: flex-start; gap: 6px; line-height: var(--line-height-normal); }
	.idle-dot { flex-shrink: 0; color: var(--accent-strong); font-size: var(--font-size-md); line-height: var(--line-height-tight); }
</style>
