<script lang="ts">
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import {
		discoverSpaceStore,
		likeNode,
		skipNode,
		queueSpaceTracks,
		branchHere,
	} from './discover_space_store';
	import type { DiscoverTrackNode } from './discover_space_types';
	import { openNodeMenu } from './discover_space_menu';

	interface Props {
		onSelectNode?: (node: DiscoverTrackNode) => void;
	}
	let { onSelectNode }: Props = $props();

	const LIST_STORAGE_KEY = 'discoverspace.list.v1';

	let collapsed = $state(readCollapsed());

	function readCollapsed(): boolean {
		try {
			return sessionStorage.getItem(LIST_STORAGE_KEY) === 'collapsed';
		} catch {
			return false;
		}
	}

	function toggleCollapsed(): void {
		collapsed = !collapsed;
		try {
			sessionStorage.setItem(LIST_STORAGE_KEY, collapsed ? 'collapsed' : 'open');
		} catch {
			// Non-persistent collapse state is fine.
		}
	}

	// Sort key priority: session rerank > shaped > normalized display score.
	function sortScore(node: DiscoverTrackNode): number {
		return node.rerankScore ?? node.shapedScore ?? node.score;
	}

	let rows = $derived(
		$discoverSpaceStore.nodes
			.filter((n) => !n.isSeed && !n.isRouteOnly)
			.toSorted((a, b) => sortScore(b) - sortScore(a))
	);
	let feedbackBusy = $derived($discoverSpaceStore.feedbackBusy);

	// Compact chip labels for the stable why-signal keys.
	const SIGNAL_LABELS: Record<string, string> = {
		key_bpm: 'Key+BPM',
		key: 'Key',
		bpm: 'Tempo',
		genre_strong: 'Genre match',
		genre: 'Genre',
		artist: 'Artist',
		energy: 'Energy',
		embedding: 'Similar',
		lastfm: 'Last.fm',
		bridge: 'Bridge',
	};

	function chips(node: DiscoverTrackNode): string[] {
		return (node.whySignals ?? [])
			.map((key) => SIGNAL_LABELS[key])
			.filter((label): label is string => Boolean(label))
			.slice(0, 2);
	}

	// Relative to the best row: shaped scores can run above 1, and an absolute
	// clamp painted most of the top of the list as a full bar.
	let topScore = $derived(rows.reduce((best, n) => Math.max(best, sortScore(n)), 0));
	function barWidth(node: DiscoverTrackNode): number {
		if (topScore <= 0) return 0;
		return Math.round(Math.max(0, sortScore(node) / topScore) * 100);
	}

	function isSuppressed(node: DiscoverTrackNode): boolean {
		return node.rerankScore !== undefined && node.rerankScore < 0.1;
	}

	function handleRowKeydown(event: KeyboardEvent, node: DiscoverTrackNode): void {
		if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			onSelectNode?.(node);
		}
	}
</script>

<aside class="ranked-list" class:collapsed aria-label="Ranked discovery results">
	<div class="list-header">
		<button
			class="collapse-btn"
			onclick={toggleCollapsed}
			aria-expanded={!collapsed}
			aria-label={collapsed ? 'Show results' : 'Hide results'}
		>
			<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 6l4 4 4-4" /></svg>
		</button>
		<span class="list-title">Results <span class="list-count">{rows.length}</span></span>
		{#if !collapsed}
			<div class="list-actions">
				<button
					class="action-btn"
					disabled={rows.length === 0}
					onclick={() => queueSpaceTracks(rows, false)}
				>
					Queue all
				</button>
				<button
					class="action-btn primary"
					disabled={rows.length === 0}
					onclick={() => queueSpaceTracks(rows, true)}
				>
					Play all
				</button>
			</div>
		{/if}
	</div>

	{#if !collapsed}
		<div class="list-body">
			{#each rows as node (node.id)}
				<div
					class="row"
					class:suppressed={isSuppressed(node)}
					role="button"
					tabindex="0"
					onclick={() => onSelectNode?.(node)}
					oncontextmenu={(e) => openNodeMenu(e, node)}
					onkeydown={(e) => handleRowKeydown(e, node)}
				>
					<div class="row-art">
						<ArtworkImage
							className="row-artwork"
							src={node.artworkUrl}
							alt={`${node.title} artwork`}
							size={320}
							fallbackText={node.title.trim().charAt(0).toUpperCase() || 'N'}
							tint={true}
						/>
					</div>
					<div class="row-main" title={node.why || undefined}>
						<span class="row-title">{node.title}</span>
						<span class="row-artist">{node.artist}</span>
						<div class="row-meta">
							{#each chips(node) as chip}
								<span class="why-chip">{chip}</span>
							{/each}
							<div class="score-bar" aria-hidden="true">
								<div class="score-fill" style:width="{barWidth(node)}%"></div>
							</div>
						</div>
					</div>
					<div class="row-actions">
						<button
							class="mini-btn"
							title="Branch here"
							aria-label="Branch into {node.title}"
							onclick={(e) => {
								e.stopPropagation();
								branchHere(node);
							}}
						>
							<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="4.5" cy="3.5" r="1.5" /><circle cx="4.5" cy="12.5" r="1.5" /><circle cx="11.5" cy="5.5" r="1.5" /><path d="M4.5 5v6M11.5 7c0 3-7 2-7 4" /></svg>
						</button>
						<button
							class="mini-btn"
							title="Add to queue"
							aria-label="Add {node.title} to queue"
							onclick={(e) => {
								e.stopPropagation();
								queueSpaceTracks([node], false);
							}}
						>
							<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3.5v9M3.5 8h9" /></svg>
						</button>
						<button
							class="mini-btn like"
							title="More like this"
							aria-label="Like {node.title}"
							disabled={feedbackBusy}
							onclick={(e) => {
								e.stopPropagation();
								likeNode(node);
							}}
						>
							<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 13.2 2.9 8.3a3 3 0 0 1 4.3-4.2l.8.8.8-.8a3 3 0 0 1 4.3 4.2Z" /></svg>
						</button>
						<button
							class="mini-btn skip"
							title="Less like this"
							aria-label="Skip {node.title}"
							disabled={feedbackBusy}
							onclick={(e) => {
								e.stopPropagation();
								skipNode(node);
							}}
						>
							<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4.5 4.5l7 7M11.5 4.5l-7 7" /></svg>
						</button>
					</div>
				</div>
			{/each}
			{#if rows.length === 0}
				<div class="empty-note">No results yet</div>
			{/if}
		</div>
	{/if}
</aside>

<style>
	.ranked-list {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
		/* Frosted, in the theme colours: a wallpaper reads through the page
		   scrim, and these panels sit on top of it. */
		background: color-mix(in srgb, var(--bg-elevated) 74%, transparent);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-lg);
		overflow: hidden;
	}
	.ranked-list.collapsed { flex: none; }

	.list-header {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		padding: var(--space-2) var(--space-3) var(--space-2) var(--space-2);
		border-bottom: 1px solid var(--border-subtle);
	}
	.ranked-list.collapsed .list-header { border-bottom: none; }
	.collapse-btn {
		display: inline-grid;
		place-items: center;
		width: 28px;
		height: 28px;
		border: none;
		border-radius: 50%;
		background: transparent;
		color: var(--text-secondary);
		cursor: pointer;
	}
	.collapse-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
	.collapse-btn svg { width: 14px; height: 14px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; transition: transform var(--motion-fast); }
	.ranked-list.collapsed .collapse-btn svg { transform: rotate(-90deg); }
	.list-title {
		flex: 1;
		min-width: 0;
		white-space: nowrap;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
	}
	.list-count {
		margin-left: 4px;
		font-weight: var(--font-weight-medium);
		color: var(--text-tertiary);
		font-variant-numeric: tabular-nums;
	}
	.list-actions { display: flex; gap: 6px; }
	.action-btn {
		min-height: 28px;
		padding: 0 var(--space-3);
		border-radius: 999px;
		border: 1px solid var(--border-muted);
		background: transparent;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		transition: background var(--motion-fast), color var(--motion-fast), filter var(--motion-fast);
	}
	.action-btn:hover:not(:disabled) { background: var(--bg-hover); color: var(--text-primary); }
	.action-btn.primary { border-color: var(--accent); background: var(--accent); color: var(--text-on-accent); }
	.action-btn.primary:hover:not(:disabled) { background: var(--accent); color: var(--text-on-accent); filter: brightness(1.1); }
	.action-btn:disabled { opacity: 0.4; cursor: not-allowed; }

	.list-body {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		padding: var(--space-1);
	}

	.row {
		position: relative;
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: 6px var(--space-2);
		border-radius: var(--radius-md);
		cursor: pointer;
		transition: background var(--motion-fast);
	}
	.row:hover, .row:focus-visible { background: var(--bg-hover); }
	.row:focus-visible { outline: 2px solid var(--accent-strong); outline-offset: -2px; }
	.row.suppressed { opacity: 0.45; }

	.row-art { flex-shrink: 0; }
	.row-art :global(.row-artwork) {
		width: 42px;
		height: 42px;
		border-radius: var(--radius-sm);
		display: block;
		object-fit: cover;
	}
	.row-art :global(.row-artwork.fallback) { display: grid; place-items: center; background: var(--bg-raised); }
	.row-art :global(.row-artwork.fallback span) { font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); }

	.row-main {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 1px;
	}
	.row-title {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		color: var(--text-primary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.row-artist {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.row-meta {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-top: 3px;
		min-width: 0;
	}
	.why-chip {
		flex-shrink: 0;
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-medium);
		color: var(--accent-strong);
		white-space: nowrap;
	}
	.why-chip + .why-chip::before { content: '/'; margin-right: 6px; color: var(--text-muted); }
	.score-bar {
		flex: 1;
		max-width: 64px;
		height: 3px;
		border-radius: 999px;
		background: var(--bg-hover);
		overflow: hidden;
	}
	.score-fill { height: 100%; border-radius: 999px; background: var(--accent); }

	/* Actions float over the row end on hover or keyboard focus, so titles keep
	   the full width at rest. Touch screens keep them in the row. */
	.row-actions {
		position: absolute;
		right: var(--space-2);
		top: 50%;
		transform: translateY(-50%);
		display: flex;
		gap: 2px;
		padding: 2px;
		border-radius: 999px;
		background: var(--bg-surface-strong);
		box-shadow: var(--panel-shadow);
		opacity: 0;
		pointer-events: none;
		transition: opacity var(--motion-fast);
	}
	.row:hover .row-actions, .row:focus-within .row-actions { opacity: 1; pointer-events: auto; }
	@media (hover: none) {
		.row-actions { position: static; transform: none; padding: 0; background: none; box-shadow: none; opacity: 1; pointer-events: auto; }
	}
	.mini-btn {
		display: inline-grid;
		place-items: center;
		width: 28px;
		height: 28px;
		border: none;
		border-radius: 50%;
		background: transparent;
		color: var(--text-secondary);
		cursor: pointer;
	}
	.mini-btn svg { width: 15px; height: 15px; fill: none; stroke: currentColor; stroke-width: 1.5; stroke-linecap: round; stroke-linejoin: round; }
	.mini-btn:hover:not(:disabled) {
		background: var(--bg-hover);
		color: var(--text-primary);
	}
	.mini-btn.like:hover:not(:disabled) { color: var(--state-favorite); }
	.mini-btn.skip:hover:not(:disabled) { color: var(--state-warning); }
	.mini-btn:disabled { opacity: 0.4; cursor: not-allowed; }

	.empty-note {
		padding: var(--space-4);
		text-align: center;
		font-size: var(--font-size-sm);
		color: var(--text-tertiary);
	}

	@container workspace (max-width: 860px) {
		.ranked-list { flex: none; max-height: 480px; }
	}
</style>
