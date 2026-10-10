<!--
	One playback-queue row, shared by the desktop queue panel and the mobile
	now-playing sheet. The parent owns play, menus and artwork fallback; pass
	`reorder` to make the row draggable (desktop only).
-->
<script lang="ts">
	import type { QueueItem } from '$lib/api/client';
	import type { DragReorderController } from '$lib/actions/drag_reorder';
	import { formatQueueSource, queueSourceSlug } from '$lib/player/queue_source';
	import { formatTrackDuration } from '$lib/utils/format';

	interface Reorder {
		row: DragReorderController['row'];
		draggable: boolean;
		dragging: boolean;
		dragOver: boolean;
	}

	interface Props {
		item: QueueItem;
		active: boolean;
		played: boolean;
		/** Resolved artwork URL (already past any failed sizes), or null for the placeholder. */
		artworkUrl: string | null;
		onArtworkError: (url: string) => void;
		onplay: () => void;
		onkeydown: (event: KeyboardEvent) => void;
		onmenu: (event: MouseEvent) => void;
		onmenubutton: (event: MouseEvent) => void;
		onartistmenu: (event: MouseEvent) => void;
		reorder?: Reorder;
	}

	let {
		item,
		active,
		played,
		artworkUrl,
		onArtworkError,
		onplay,
		onkeydown,
		onmenu,
		onmenubutton,
		onartistmenu,
		reorder,
	}: Props = $props();

	const noDrag: DragReorderController['row'] = () => ({ update() {}, destroy() {} });
	let rowAction = $derived(reorder?.row ?? noDrag);
	let artistId = $derived(item.track.artist_id);
	let isPending = $derived(item.is_pending === true);

	function stopPropagation(event: Event) {
		event.stopPropagation();
	}
</script>

<div
	role="listitem"
	class="queue-row"
	class:active
	class:played
	class:dragging={reorder?.dragging ?? false}
	class:drag-over={reorder?.dragOver ?? false}
	class:pending={isPending}
	title={isPending ? 'Resolving on TIDAL...' : undefined}
	data-queue-item-id={item.id}
	draggable={reorder?.draggable ?? false}
	oncontextmenu={onmenu}
	use:rowAction={item.id}
>
	<!-- Full-bleed hit target: clicking anywhere on the row that isn't an
	     interactive child plays/jumps to this track. This is a div, NOT a
	     button, on purpose: a <button> is an interactive element and swallows
	     the row's native HTML5 dragstart, so the row could only be dragged by
	     the 12px grip. role/tabindex keep it keyboard- and screen-reader-operable. -->
	<div
		class="queue-row-hit"
		role="button"
		tabindex={0}
		aria-label={isPending ? `Play ${item.track.title} (resolving)` : `Play ${item.track.title}`}
		onclick={onplay}
		{onkeydown}
	></div>
	{#if reorder}
		<span class="queue-grip" aria-hidden="true" title="Drag to reorder">⋮⋮</span>
	{/if}
	<div class="queue-art-wrap" title={formatQueueSource(item.source)}>
		{#if isPending}
			<div class="queue-art placeholder pending-art" title="Resolving track...">
				<span class="queue-spinner" aria-hidden="true"></span>
			</div>
		{:else if artworkUrl}
			<img class="queue-art" src={artworkUrl} alt="" onerror={() => onArtworkError(artworkUrl)} />
		{:else}
			<div class="queue-art placeholder">♫</div>
		{/if}
		<span class="queue-source-dot source-{queueSourceSlug(item.source)}" aria-hidden="true"></span>
	</div>

	<div class="queue-meta">
		<span class="queue-title">{item.track.title}</span>
		{#if isPending}
			<span class="queue-artist pending-label">
				<span class="queue-inline-spinner" aria-hidden="true"></span>
				Resolving on TIDAL...
			</span>
		{:else if artistId && artistId > 0}
			<a
				class="queue-artist"
				href="/artists/{artistId}"
				onclick={stopPropagation}
				oncontextmenu={onartistmenu}
			>{item.track.artist_name ?? 'Unknown artist'}</a>
		{:else}
			<span class="queue-artist">{item.track.artist_name ?? 'Unknown artist'}</span>
		{/if}
	</div>

	<div class="queue-side">
		<span class="queue-time">{formatTrackDuration(item.track.duration_ms)}</span>
		<button
			class="queue-overflow"
			aria-label="More actions"
			title="More actions"
			onclick={onmenubutton}
		>⋯</button>
	</div>
</div>

<style>
	.queue-row {
		position: relative;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 6px 8px;
		border: 1px solid color-mix(in srgb, var(--instrument-border) 46%, transparent);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--instrument-surface) 78%, transparent);
		transition:
			border-color var(--motion-fast),
			background var(--motion-fast),
			transform var(--motion-fast);
	}

	/* Full-bleed click target sits behind the row content. Non-interactive
	   content (art, title, time) has pointer-events:none so clicks fall through
	   to it; interactive children (grip, artist link, overflow) re-enable. */
	.queue-row-hit {
		position: absolute;
		inset: 0;
		z-index: 0;
		margin: 0;
		padding: 0;
		border: none;
		background: transparent;
		border-radius: inherit;
		cursor: pointer;
	}

	.queue-row-hit:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: -2px;
	}

	.queue-row > .queue-grip,
	.queue-row > .queue-art-wrap,
	.queue-row > .queue-meta,
	.queue-row > .queue-side {
		position: relative;
		z-index: 1;
	}

	.queue-art-wrap,
	.queue-meta,
	.queue-time {
		pointer-events: none;
	}

	.queue-grip,
	.queue-meta .queue-artist[href],
	.queue-overflow {
		pointer-events: auto;
	}

	.queue-row:hover,
	.queue-row:focus-within {
		border-color: color-mix(in srgb, var(--instrument-border) 72%, transparent);
		background: color-mix(in srgb, var(--instrument-surface-strong) 86%, transparent);
		transform: translateY(-1px);
	}

	.queue-row.active .queue-title {
		color: var(--playing);
	}

	.queue-row.active {
		border-color: color-mix(in srgb, var(--playing) 28%, transparent);
		background: var(--playing-soft);
	}

	.queue-row.active::before {
		content: '';
		position: absolute;
		left: 0;
		top: 10px;
		bottom: 10px;
		width: 2px;
		border-radius: 2px;
		background: var(--playing);
	}

	.queue-row.played {
		opacity: 0.56;
		background: color-mix(in srgb, var(--instrument-surface) 48%, transparent);
	}

	.queue-row.played:hover,
	.queue-row.played:focus-within {
		opacity: 0.78;
	}

	.queue-row.played .queue-grip {
		visibility: hidden;
	}

	.queue-row.dragging {
		opacity: 0.4;
		cursor: grabbing;
	}

	/* The dropped row lands at the target's index, i.e. above it, so the
	   accent line sits on the target's top edge to read as "drops here". */
	.queue-row.drag-over {
		border-color: var(--accent-line);
		background: color-mix(in srgb, var(--accent-soft) 55%, transparent);
		box-shadow: inset 0 2px 0 var(--accent-strong);
	}

	.queue-row.pending {
		cursor: default;
		opacity: 0.78;
	}

	.queue-row.pending:hover,
	.queue-row.pending:focus-within {
		transform: none;
	}

	.queue-row.pending .queue-title {
		color: var(--text-secondary);
	}

	.queue-art.placeholder.pending-art {
		opacity: 0.7;
	}

	.queue-spinner {
		width: 16px;
		height: 16px;
		border-radius: 50%;
		border: 2px solid var(--border-subtle, rgba(255, 255, 255, 0.15));
		border-top-color: var(--text-secondary, rgba(255, 255, 255, 0.7));
		animation: queue-spinner-spin 0.9s linear infinite;
	}

	@keyframes queue-spinner-spin {
		to { transform: rotate(360deg); }
	}

	.queue-grip {
		flex-shrink: 0;
		width: 12px;
		text-align: center;
		font-size: var(--font-size-xs);
		line-height: 1;
		color: var(--text-tertiary);
		cursor: grab;
		opacity: 0.35;
		transition: opacity var(--motion-fast);
		user-select: none;
	}

	.queue-row:hover .queue-grip,
	.queue-row:focus-within .queue-grip {
		opacity: 0.8;
	}

	.queue-row.dragging .queue-grip {
		cursor: grabbing;
	}

	.queue-art-wrap {
		position: relative;
		flex-shrink: 0;
		line-height: 0;
	}

	.queue-art {
		width: 42px;
		height: 42px;
		border-radius: 12px;
		object-fit: cover;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		display: block;
	}

	.queue-art.placeholder {
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
	}

	/* The dot in the bottom-right of queue artwork encodes where the track came
	   from; its colours live in app.css so the legend on the automix page can
	   reuse them. Tooltip on .queue-art-wrap names the source. */

	.queue-meta {
		min-width: 0;
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.queue-title {
		font-weight: var(--font-weight-semibold);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
		margin: 0;
		/* Two-line clamp lets long titles breathe instead of chopping words. */
		display: -webkit-box;
		-webkit-box-orient: vertical;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		overflow: hidden;
		overflow-wrap: anywhere;
		word-break: break-word;
	}

	.queue-artist {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-snug);
		text-decoration: none;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 100%;
	}

	a.queue-artist {
		cursor: pointer;
	}

	a.queue-artist:hover {
		color: var(--text-primary);
		text-decoration: underline;
	}

	.queue-artist.pending-label {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		color: var(--text-tertiary);
	}

	.queue-inline-spinner {
		width: 10px;
		height: 10px;
		border-radius: 999px;
		border: 1.5px solid var(--border-subtle, rgba(255, 255, 255, 0.15));
		border-top-color: var(--text-secondary, rgba(255, 255, 255, 0.7));
		animation: queue-spinner-spin 0.9s linear infinite;
		flex-shrink: 0;
	}

	.queue-time {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.queue-side {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 6px;
		flex-shrink: 0;
		margin-left: auto;
	}

	/* Single overflow button replaces the old cluster of hover pills: low-key by
	   default, brightens on row hover/focus. The context menu holds every action
	   (play next, favourite, radio, remove), so the row stays calm. */
	.queue-overflow {
		width: 28px;
		height: 28px;
		padding: 0;
		display: inline-grid;
		place-items: center;
		border-radius: 999px;
		border: 1px solid transparent;
		background: transparent;
		color: var(--text-tertiary);
		font-size: var(--font-size-md);
		line-height: 1;
		cursor: pointer;
		opacity: 0.55;
		transition: background var(--motion-fast), color var(--motion-fast),
			border-color var(--motion-fast), opacity var(--motion-fast);
	}

	.queue-row:hover .queue-overflow,
	.queue-row:focus-within .queue-overflow {
		opacity: 1;
	}

	@media (hover: none) {
		.queue-overflow { opacity: 1; }
	}

	.queue-overflow:hover {
		background: color-mix(in srgb, var(--instrument-surface-strong) 92%, transparent);
		border-color: color-mix(in srgb, var(--instrument-border) 70%, transparent);
		color: var(--text-primary);
	}

	/* Small phones: queue touch tweaks. */
	@media (max-width: 760px) {
		.queue-row { align-items: flex-start; }
		.queue-side { align-items: flex-end; }
		.queue-time { display: none; }
		/* Overflow stays tappable without a hover state on touch. */
		.queue-overflow { opacity: 1; }
	}

	/* Honor OS-level motion-reduction: flatten the lift and the spinners. */
	@media (prefers-reduced-motion: reduce) {
		.queue-row,
		.queue-row:hover,
		.queue-row:focus-within {
			transform: none;
		}
		.queue-spinner,
		.queue-inline-spinner {
			animation: none;
		}
	}
</style>
