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

	function barWidth(node: DiscoverTrackNode): number {
		return Math.round(Math.min(1, Math.max(0, sortScore(node))) * 100);
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
		<button class="collapse-btn" onclick={toggleCollapsed} aria-expanded={!collapsed}>
			{collapsed ? '◂' : '▸'}
		</button>
		{#if !collapsed}
			<span class="list-title">Results ({rows.length})</span>
			<div class="list-actions">
				<button
					class="action-btn primary"
					disabled={rows.length === 0}
					onclick={() => queueSpaceTracks(rows, true)}
				>
					Play all
				</button>
				<button
					class="action-btn"
					disabled={rows.length === 0}
					onclick={() => queueSpaceTracks(rows, false)}
				>
					Queue all
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
					onkeydown={(e) => handleRowKeydown(e, node)}
				>
					<div class="row-art">
						<ArtworkImage
							className="row-artwork"
							src={node.artworkUrl}
							alt={`${node.title} artwork`}
							size={320}
							fallbackText={node.title.slice(0, 2).toUpperCase()}
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
							⑂
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
							+
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
							♥
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
							✕
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
		width: 300px;
		max-height: 100%;
		background: rgba(0, 0, 0, 0.5);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		border: 1px solid var(--panel-border);
		border-radius: 12px;
		overflow: hidden;
	}
	.ranked-list.collapsed {
		width: auto;
	}
	.list-header {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px 10px;
		border-bottom: 1px solid var(--panel-border);
	}
	.ranked-list.collapsed .list-header {
		border-bottom: none;
	}
	.collapse-btn {
		border: none;
		background: transparent;
		color: rgba(255, 255, 255, 0.6);
		cursor: pointer;
		font-size: var(--font-size-sm);
		padding: 2px 4px;
	}
	.list-title {
		flex: 1;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
		color: rgba(255, 255, 255, 0.85);
		white-space: nowrap;
	}
	.list-actions {
		display: flex;
		gap: 4px;
	}
	.action-btn {
		padding: 3px 10px;
		border-radius: 999px;
		border: 1px solid var(--panel-border);
		background: transparent;
		color: rgba(255, 255, 255, 0.7);
		font-size: var(--font-size-xs);
		cursor: pointer;
		white-space: nowrap;
	}
	.action-btn.primary {
		background: rgba(124, 128, 255, 0.25);
		color: rgba(255, 255, 255, 0.95);
	}
	.action-btn:disabled {
		opacity: 0.4;
		cursor: default;
	}
	.list-body {
		flex: 1;
		overflow-y: auto;
		padding: 4px;
	}
	.row {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 5px 6px;
		border-radius: 8px;
		cursor: pointer;
		transition: background 0.12s, opacity 0.2s;
	}
	.row:hover,
	.row:focus-visible {
		background: rgba(255, 255, 255, 0.06);
		outline: none;
	}
	.row.suppressed {
		opacity: 0.35;
	}
	.row-art {
		width: 40px;
		height: 40px;
		flex-shrink: 0;
		border-radius: 6px;
		overflow: hidden;
	}
	.row-art :global(.row-artwork) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.row-main {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 1px;
	}
	.row-title {
		font-size: var(--font-size-xs);
		color: rgba(255, 255, 255, 0.9);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.row-artist {
		font-size: var(--font-size-2xs);
		color: rgba(255, 255, 255, 0.5);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.row-meta {
		display: flex;
		align-items: center;
		gap: 4px;
		margin-top: 2px;
	}
	.why-chip {
		font-size: var(--font-size-2xs);
		padding: 0 6px;
		border-radius: 999px;
		background: rgba(124, 128, 255, 0.18);
		color: rgba(200, 202, 255, 0.9);
		white-space: nowrap;
	}
	.score-bar {
		flex: 1;
		height: 3px;
		min-width: 24px;
		border-radius: 2px;
		background: rgba(255, 255, 255, 0.08);
		overflow: hidden;
	}
	.score-fill {
		height: 100%;
		border-radius: 2px;
		background: rgba(124, 128, 255, 0.7);
	}
	.row-actions {
		display: flex;
		gap: 2px;
		flex-shrink: 0;
	}
	.mini-btn {
		width: 24px;
		height: 24px;
		border-radius: 6px;
		border: none;
		background: transparent;
		color: rgba(255, 255, 255, 0.45);
		font-size: var(--font-size-xs);
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.mini-btn:hover:not(:disabled) {
		background: rgba(255, 255, 255, 0.08);
		color: rgba(255, 255, 255, 0.9);
	}
	.mini-btn.like:hover:not(:disabled) {
		color: rgba(255, 130, 170, 0.95);
	}
	.mini-btn.skip:hover:not(:disabled) {
		color: rgba(255, 180, 120, 0.95);
	}
	.mini-btn:disabled {
		opacity: 0.4;
		cursor: default;
	}
	.empty-note {
		padding: 16px;
		text-align: center;
		font-size: var(--font-size-xs);
		color: rgba(255, 255, 255, 0.35);
	}
</style>
