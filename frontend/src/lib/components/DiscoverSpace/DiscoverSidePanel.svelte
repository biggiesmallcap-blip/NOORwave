<script lang="ts">
	import { lockSeed, setRadioRoute, discoverSpaceStore } from './discover_space_store';
	import { REASON_LABELS, REASON_EXPLANATIONS, SOURCE_LABELS, SIDE_PANEL_ACTIONS, ERROR_TOASTS, LENS_LABELS, LENS_DESCRIPTIONS } from './discover_space_story';
	import type { DiscoverTrackNode, DiscoverReason } from './discover_space_types';
	import { api, getApiBase, authFetch } from '$lib/api/client';
	import { showToast } from '$lib/stores/toast';

	interface Props {
		node: DiscoverTrackNode | null;
		seedNode?: DiscoverTrackNode | null;
		onAddToPlaylist?: (node: DiscoverTrackNode) => void;
	}
	let { node, seedNode = null, onAddToPlaylist }: Props = $props();

	let isStartingRadio = $state(false);
	let isHiding = $state(false);

	async function handlePlayNow() {
		if (!node) return;
		try {
			const { playTrackNow } = await import('$lib/stores/player');
			await playTrackNow(node.trackId);
		} catch { /* existing toast pattern */ }
	}

	async function handlePlayNext() {
		if (!node) return;
		try {
			const { playTrackNext } = await import('$lib/stores/player');
			await playTrackNext(node.trackId);
		} catch { /* silent */ }
	}

	async function handleStartRadioHere() {
		if (!node || isStartingRadio) return;
		isStartingRadio = true;
		try {
			const { startSongRadio } = await import('$lib/stores/player');
			await startSongRadio(node.trackId);
		} catch {
			showToast(ERROR_TOASTS.radioRouteFailed, 'error');
		} finally {
			isStartingRadio = false;
		}
	}

	function handleLockAsAnchor() {
		if (!node) return;
		lockSeed(node.trackId);
	}

	function handleAddToPlaylist() {
		if (!node) return;
		onAddToPlaylist?.(node);
	}

	async function handleHideFromRadio() {
		if (!node || isHiding) return;
		isHiding = true;
		// Optimistic: mark hidden locally (node remains visible but dimmed in canvas)
		try {
			const apiBase = getApiBase();
			await authFetch(`${apiBase}/api/discovery/feedback`, {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ track_id: node.trackId, action: 'dismiss' }),
			});
		} catch {
			// Roll back optimistic
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
				<img class="artwork" src={node.artworkUrl} alt="{node.title} artwork" width="56" height="56" />
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
			<button class="action-btn primary" onclick={handlePlayNow} aria-label="Play {node.title} now">
				{SIDE_PANEL_ACTIONS.playNow}
			</button>
			<button class="action-btn" onclick={handlePlayNext} aria-label="Play {node.title} next">
				{SIDE_PANEL_ACTIONS.playNext}
			</button>
			<button
				class="action-btn"
				onclick={handleStartRadioHere}
				disabled={isStartingRadio}
				aria-label="Start radio from {node.title}"
			>
				{isStartingRadio ? 'Starting…' : SIDE_PANEL_ACTIONS.startRadioHere}
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
					<img class="idle-artwork" src={seedNode.artworkUrl} alt="" aria-hidden="true" width="40" height="40" />
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
		border-left: 1px solid rgba(255, 255, 255, 0.06);
	}
	.panel-empty {
		height: 100%;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		color: rgba(255,255,255,0.2);
		font-size: 0.82rem;
	}
	.panel-empty-icon { font-size: 2rem; opacity: 0.3; }

	.panel-header { display: flex; gap: 10px; align-items: flex-start; }
	.artwork { width: 56px; height: 56px; border-radius: 6px; object-fit: cover; flex-shrink: 0; }
	.artwork-placeholder { width: 56px; height: 56px; border-radius: 6px; background: rgba(255,255,255,0.05); flex-shrink: 0; }
	.panel-title-group { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
	.panel-title { font-weight: 600; font-size: 0.9rem; color: rgba(255,255,255,0.95); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.panel-artist { font-size: 0.78rem; color: rgba(255,255,255,0.55); }
	.panel-album { font-size: 0.72rem; color: rgba(255,255,255,0.35); }

	.fingerprint { display: flex; flex-wrap: wrap; gap: 6px; }
	.fp-chip {
		background: rgba(255,255,255,0.05);
		border: 1px solid rgba(255,255,255,0.08);
		border-radius: 6px;
		padding: 5px 8px;
		display: flex;
		flex-direction: column;
		gap: 3px;
		flex: 1;
		min-width: 60px;
	}
	.fp-chip.narrow { min-width: 44px; flex: 0; }
	.fp-label { font-size: 0.62rem; text-transform: uppercase; letter-spacing: 0.06em; color: rgba(255,255,255,0.35); }
	.fp-value { font-size: 0.82rem; font-weight: 600; color: rgba(255,255,255,0.85); font-variant-numeric: tabular-nums; }
	.fp-bar { height: 3px; background: rgba(255,255,255,0.08); border-radius: 999px; overflow: hidden; }
	.fp-fill { height: 100%; background: rgba(124,128,255,0.6); border-radius: 999px; }

	.why-section { display: flex; flex-direction: column; gap: 6px; }
	.why-heading { font-size: 0.68rem; text-transform: uppercase; letter-spacing: 0.1em; color: rgba(255,255,255,0.3); }
	.why-reason { display: flex; flex-direction: column; gap: 3px; }
	.reason-pill { font-size: 0.7rem; font-weight: 600; color: rgba(124,128,255,0.9); }
	.reason-explanation { font-size: 0.75rem; color: rgba(255,255,255,0.5); line-height: 1.45; }
	.extra-reasons { display: flex; flex-wrap: wrap; gap: 4px; }
	.reason-mini {
		padding: 1px 6px;
		border-radius: 4px;
		background: rgba(124,128,255,0.1);
		color: rgba(160,165,255,0.7);
		font-size: 0.65rem;
	}
	.conf-row { display: flex; align-items: center; gap: 6px; }
	.conf-bar { flex: 1; height: 3px; background: rgba(255,255,255,0.07); border-radius: 999px; overflow: hidden; }
	.conf-fill { height: 100%; background: rgba(124,128,255,0.6); border-radius: 999px; }
	.conf-label { font-size: 0.65rem; color: rgba(255,255,255,0.3); white-space: nowrap; }

	.meta-row { display: flex; flex-wrap: wrap; gap: 4px; }
	.meta-tag { padding: 2px 7px; border-radius: 4px; font-size: 0.66rem; }
	.meta-tag.genre { background: rgba(80,180,100,0.1); color: rgba(120,200,140,0.8); }
	.meta-tag.source { background: rgba(100,120,220,0.12); color: rgba(160,170,255,0.75); }
	.meta-tag.cold { background: rgba(60,60,80,0.3); color: rgba(140,140,160,0.6); }

	.actions { display: flex; flex-direction: column; gap: 5px; }
	.action-btn {
		padding: 8px 12px;
		border-radius: 8px;
		border: 1px solid rgba(255,255,255,0.08);
		background: rgba(255,255,255,0.04);
		color: rgba(255,255,255,0.75);
		font-size: 0.78rem;
		cursor: pointer;
		text-align: left;
		transition: background 0.12s, color 0.12s;
	}
	.action-btn:hover:not(:disabled) { background: rgba(255,255,255,0.09); color: rgba(255,255,255,0.95); }
	.action-btn.primary {
		background: rgba(124,128,255,0.18);
		border-color: rgba(124,128,255,0.3);
		color: rgba(200,202,255,0.95);
	}
	.action-btn.primary:hover:not(:disabled) { background: rgba(124,128,255,0.28); }
	.action-btn.active { color: rgba(255,200,50,0.9); border-color: rgba(255,200,50,0.3); }
	.action-btn.destructive { color: rgba(255,100,100,0.6); }
	.action-btn.destructive:hover:not(:disabled) { color: rgba(255,100,100,0.9); }
	.action-btn:disabled { opacity: 0.4; cursor: not-allowed; }

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
	.idle-artwork {
		width: 40px; height: 40px; border-radius: 5px; object-fit: cover; flex-shrink: 0;
		box-shadow: 0 0 0 1px rgba(124,128,255,0.3), 0 0 10px rgba(124,128,255,0.15);
	}
	.idle-artwork-placeholder {
		width: 40px; height: 40px; border-radius: 5px; flex-shrink: 0;
		background: rgba(124,128,255,0.12);
		border: 1px solid rgba(124,128,255,0.25);
	}
	.idle-anchor-info { flex: 1; min-width: 0; }
	.idle-anchor-label {
		font-size: 0.6rem; text-transform: uppercase; letter-spacing: 0.1em;
		color: rgba(124,128,255,0.6); margin-bottom: 2px;
	}
	.idle-anchor-title { font-size: 0.88rem; font-weight: 600; color: rgba(255,255,255,0.9); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.idle-anchor-artist { font-size: 0.75rem; color: rgba(255,255,255,0.45); }

	.idle-genres { display: flex; flex-wrap: wrap; gap: 4px; }
	.idle-genre-tag {
		padding: 2px 7px; border-radius: 4px; font-size: 0.66rem;
		background: rgba(80,180,100,0.1); color: rgba(120,200,140,0.8);
	}

	.idle-lens {
		display: flex; flex-direction: column; gap: 3px;
		padding: 8px 10px;
		background: rgba(255,255,255,0.03);
		border: 1px solid rgba(255,255,255,0.06);
		border-radius: 6px;
	}
	.idle-lens-name { font-size: 0.65rem; text-transform: uppercase; letter-spacing: 0.08em; color: rgba(124,128,255,0.7); }
	.idle-lens-desc { font-size: 0.74rem; color: rgba(255,255,255,0.42); line-height: 1.45; }

	.idle-instructions { display: flex; flex-direction: column; gap: 5px; padding-top: 2px; }
	.idle-instr-row { font-size: 0.73rem; color: rgba(255,255,255,0.32); display: flex; align-items: flex-start; gap: 6px; line-height: 1.4; }
	.idle-dot { flex-shrink: 0; color: rgba(124,128,255,0.45); font-size: 1rem; line-height: 1.1; }
</style>
