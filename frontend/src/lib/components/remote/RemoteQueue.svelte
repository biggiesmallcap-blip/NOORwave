<script lang="ts">
	import { api, type QueueItem, type Track } from '$lib/api/client';
	import {
		clearQueue,
		moveQueueTrackNext,
		playTidalTrackNow,
		playTrackNow,
		refreshPlaybackState,
		removeTrackFromQueue,
		restoreQueueItems
	} from '$lib/stores/player';
	import { pendingUndo, consumeUndo } from '$lib/stores/queue_undo';
	import {
		tidalArtworkFallbackSizes,
		upscaleTidalArtwork,
		type TidalArtworkSize
	} from '$lib/utils/artwork';
	import { formatTrackDuration } from '$lib/utils/format';
	import { queueItemToTidalPlayable } from '$lib/utils/track';
	import {
		buildTidalTrackMenu,
		buildTrackMenu,
		type MenuTrack
	} from '$lib/player/track_menu';
	import { isQueueItemActive } from '$lib/player/queue_active';
	import { openActionSheet } from '$lib/remote/action_sheet';
	import { hapticAccent, hapticCommit, hapticTap } from '$lib/remote/haptics';
	import { longPress } from '$lib/remote/long_press';

	let {
		queue,
		currentTrack: current = null,
		currentQueueItemId = null
	}: { queue: QueueItem[]; currentTrack?: Track | null; currentQueueItemId?: number | null } =
		$props();

	// Optimistic reorder copy. While a drag is in progress we render this
	// instead of the prop so the row follows the finger. Reset to the prop
	// whenever `queue` changes from outside (WS push, etc.) AND we aren't
	// mid-drag.
	let displayQueue = $state<QueueItem[]>([]);
	$effect(() => {
		if (dragState.active) return;
		displayQueue = queue;
	});
	let failedArtworkUrls = $state<Record<string, boolean>>({});

	const ROW_HEIGHT = 64; // matches min-height + gap below; used for index math
	const dragState = $state({
		active: false,
		itemId: 0,
		startIndex: 0,
		currentIndex: 0,
		offset: 0,
		startY: 0,
	});

	function canPlay(item: QueueItem): boolean {
		return item.is_pending !== true && (item.track.id > 0 || queueItemToTidalPlayable(item) != null);
	}

	function isCurrent(item: QueueItem): boolean {
		return isQueueItemActive(item, current, currentQueueItemId, displayQueue);
	}

	function openRowMenu(item: QueueItem) {
		if (item.is_pending) {
			// Pending rows have the artist/title COALESCEd into track.* by the
			// server mapper, so we can render those directly.
			openActionSheet({
				title: item.track.title,
				subtitle: item.track.artist_name,
				items: buildTrackMenu(
					{ id: item.track.id, title: item.track.title },
					{ queueItemId: item.id, isPending: true, remoteRoutes: true }
				),
			});
			return;
		}
		const t = item.track;
		const tidal = queueItemToTidalPlayable(item);
		if (tidal != null) {
			openActionSheet({
				title: t.title,
				subtitle: t.artist_name,
				items: buildTidalTrackMenu(tidal, { inQueue: true, remoteRoutes: true })
			});
			return;
		}
		const menuTrack: MenuTrack = {
			id: t.id,
			title: t.title,
			artist_id: t.artist_id ?? null,
			artist_name: t.artist_name ?? null,
			album_id: t.album_id ?? null,
			album_title: t.album_title ?? null,
			is_favorite: t.is_favorite ?? false
		};
		openActionSheet({
			title: t.title,
			subtitle: t.artist_name,
			items: buildTrackMenu(menuTrack, { queueItemId: item.id, remoteRoutes: true })
		});
	}

	function onClear() {
		hapticAccent();
		void clearQueue();
	}

	async function onUndoClear() {
		const restorable = consumeUndo();
		if (!restorable) return;
		hapticCommit();
		await restoreQueueItems(restorable);
	}

	async function onPlayRow(item: QueueItem) {
		hapticTap();
		const tidal = queueItemToTidalPlayable(item);
		if (tidal != null && item.id < 0) {
			await playTidalTrackNow(tidal);
		} else {
			await playTrackNow(item.track.id);
		}
	}

	async function onMoveNext(item: QueueItem, event: Event) {
		event.stopPropagation();
		hapticTap();
		await moveQueueTrackNext(item.id);
	}

	async function onRemove(item: QueueItem, event: Event) {
		event.stopPropagation();
		hapticTap();
		await removeTrackFromQueue(item.id);
	}

	function queueArtwork(item: QueueItem, size: TidalArtworkSize = 320): string | null {
		if (item.is_pending) return null;
		const rawUrl = item.track.artwork_url;
		if (!rawUrl) return null;
		for (const fallbackSize of tidalArtworkFallbackSizes(rawUrl, size)) {
			const renderedUrl = upscaleTidalArtwork(rawUrl, fallbackSize);
			if (renderedUrl && !failedArtworkUrls[renderedUrl]) return renderedUrl;
		}
		return null;
	}

	function markArtworkFailed(renderedUrl: string | null) {
		if (!renderedUrl) return;
		failedArtworkUrls = { ...failedArtworkUrls, [renderedUrl]: true };
	}

	// Drag-reorder
	// Pointerdown on a row's drag handle starts a reorder gesture. We let the
	// row follow the finger via a CSS transform and recompute the target index
	// from the cumulative y-offset. On release we fire `api.moveQueueTrack` for
	// the new position. Optimistic: the local order updates immediately and
	// reverts only if the server returns an error.
	function onDragStart(event: PointerEvent, index: number, item: QueueItem) {
		if (event.pointerType === 'mouse' && event.button !== 0) return;
		if (item.is_pending) return; // pending rows have no real track to reorder yet
		event.stopPropagation();
		dragState.active = true;
		dragState.itemId = item.id;
		dragState.startIndex = index;
		dragState.currentIndex = index;
		dragState.offset = 0;
		dragState.startY = event.clientY;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
		hapticTap();
	}

	function onDragMove(event: PointerEvent) {
		if (!dragState.active) return;
		const dy = event.clientY - dragState.startY;
		dragState.offset = dy;
		const targetIndex = clampIndex(
			dragState.startIndex + Math.round(dy / ROW_HEIGHT),
			displayQueue.length
		);
		if (targetIndex !== dragState.currentIndex) {
			dragState.currentIndex = targetIndex;
			displayQueue = reorder(displayQueue, dragState.startIndex, targetIndex);
			// Recenter the gesture so subsequent movement reads from the new
			// row position; otherwise the row drifts away from the finger.
			dragState.startIndex = targetIndex;
			dragState.startY = event.clientY;
			dragState.offset = 0;
			hapticTap();
		}
	}

	async function onDragEnd(event: PointerEvent) {
		if (!dragState.active) return;
		const finalIndex = dragState.currentIndex;
		const itemId = dragState.itemId;
		const originalIndex = queue.findIndex((q) => q.id === itemId);
		dragState.active = false;
		dragState.offset = 0;
		(event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
		if (finalIndex === originalIndex || originalIndex < 0) return;
		hapticCommit();
		try {
			const result = await api.moveQueueTrack(itemId, finalIndex);
			// Sync state from the server so the optimistic copy matches truth.
			displayQueue = result.queue;
			await refreshPlaybackState();
		} catch {
			// On failure, revert to the prop snapshot.
			displayQueue = queue;
		}
	}

	function clampIndex(value: number, length: number): number {
		if (value < 0) return 0;
		if (value > length - 1) return length - 1;
		return value;
	}

	function reorder(list: QueueItem[], from: number, to: number): QueueItem[] {
		if (from === to) return list;
		const copy = list.slice();
		const [removed] = copy.splice(from, 1);
		copy.splice(to, 0, removed);
		return copy;
	}
</script>

<section class="remote-queue" aria-label="Up next">
	<header>
		<h2>Up next</h2>
		<div class="remote-queue-header-actions">
			<span class="remote-queue-count">{displayQueue.length}</span>
			<button
				class="remote-queue-clear"
				type="button"
				disabled={displayQueue.length === 0}
				aria-label="Clear queue"
				onclick={onClear}
			>
				Clear
			</button>
		</div>
	</header>

	{#if $pendingUndo}
		<div class="remote-queue-undo" role="status">
			<span>Cleared {$pendingUndo.count} {$pendingUndo.count === 1 ? 'track' : 'tracks'}</span>
			<button
				class="remote-queue-undo-btn"
				type="button"
				onclick={() => void onUndoClear()}
			>Undo</button>
		</div>
	{/if}

	{#if displayQueue.length === 0}
		<p class="remote-empty">Nothing is lined up.</p>
	{:else}
		<div class="remote-queue-list">
			{#each displayQueue.slice(0, 20) as item, index (item.id)}
				{@const queueArt = queueArtwork(item)}
				<div
					class="remote-queue-row"
					class:pending={item.is_pending}
					class:current={isCurrent(item)}
					class:dragging={dragState.active && dragState.itemId === item.id}
					style="--drag-y: {dragState.active && dragState.itemId === item.id
						? dragState.offset
						: 0}px;"
				>
					<button
						class="remote-queue-play"
						type="button"
						disabled={!canPlay(item) || isCurrent(item)}
						aria-label={isCurrent(item) ? 'Now playing' : 'Play queued track'}
						use:longPress={() => openRowMenu(item)}
						onclick={() => void onPlayRow(item)}
					>
						{#if queueArt}
							<img
								src={queueArt}
								alt=""
								loading="lazy"
								decoding="async"
								onerror={() => markArtworkFailed(queueArt)}
							/>
						{:else}
							<span class="remote-queue-thumb-empty" aria-hidden="true">NOOR</span>
						{/if}
						<span class="remote-queue-copy">
							<strong>
								{#if isCurrent(item)}<em class="remote-queue-now" aria-hidden="true">Now</em
									>{/if}{item.track.title}
							</strong>
							<small>{item.track.artist_name ?? 'Unknown artist'}</small>
						</span>
						<small class="remote-queue-duration">{formatTrackDuration(item.track.duration_ms)}</small>
					</button>
					<div class="remote-queue-actions">
						<button
							class="remote-queue-chip"
							type="button"
							disabled={item.is_pending === true || isCurrent(item)}
							aria-label="Move to play next"
							onclick={(e) => void onMoveNext(item, e)}
						>
							Next
						</button>
						<button
							class="remote-queue-chip remove"
							type="button"
							disabled={item.is_pending === true}
							aria-label="Remove from queue"
							onclick={(e) => void onRemove(item, e)}
						>
							Remove
						</button>
						<button
							class="remote-queue-drag"
							type="button"
							aria-label="Drag to reorder"
							disabled={item.is_pending === true}
							onpointerdown={(e) => onDragStart(e, index, item)}
							onpointermove={onDragMove}
							onpointerup={onDragEnd}
							onpointercancel={onDragEnd}
						>
							<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
								<path
									d="M9 6h.01M9 12h.01M9 18h.01M15 6h.01M15 12h.01M15 18h.01"
									stroke="currentColor"
									stroke-width="2.4"
									stroke-linecap="round"
								/>
							</svg>
						</button>
					</div>
				</div>
			{/each}
		</div>
	{/if}
</section>

<style>
	.remote-queue,
	.remote-queue-list {
		display: grid;
		gap: 8px;
	}

	/* content-visibility: auto on queue rows was thrashing iOS Safari scroll
	   on long queues - same issue as the library list. Lazy-loaded images
	   are enough for the perf budget here. */

	.remote-queue {
		gap: 12px;
	}

	.remote-queue header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
	}

	.remote-queue h2,
	.remote-empty {
		margin: 0;
	}

	.remote-queue-row {
		display: flex;
		align-items: center;
		gap: 8px;
		min-height: 56px;
		transform: translate3d(0, var(--drag-y, 0px), 0);
		transition: transform 180ms cubic-bezier(0.22, 1.2, 0.36, 1);
	}

	.remote-queue-row.dragging {
		z-index: 2;
		transition: none;
		background: var(--surface-1);
		border-radius: 10px;
		box-shadow: 0 14px 30px rgba(0, 0, 0, 0.35);
	}

	.remote-queue-row.current {
		opacity: 0.85;
	}

	.remote-queue-play {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: 10px;
		text-align: left;
		padding: 4px 0;
	}

	.remote-queue-play img,
	.remote-queue-thumb-empty {
		width: 44px;
		height: 44px;
		border-radius: 6px;
		object-fit: cover;
		flex-shrink: 0;
	}

	.remote-queue-thumb-empty {
		display: grid;
		place-items: center;
		background: var(--surface-1);
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-queue-copy {
		min-width: 0;
		display: grid;
		flex: 1;
	}

	.remote-queue-copy strong,
	.remote-queue-copy small {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-queue-copy small {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-queue-duration {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
		flex-shrink: 0;
	}

	.remote-queue-actions {
		display: flex;
		gap: 6px;
		flex-shrink: 0;
	}

	.remote-queue-chip {
		min-height: 40px;
		min-width: 40px;
		padding: 0 12px;
		border-radius: 8px;
		background: var(--surface-1);
		color: var(--text-primary);
		font-size: var(--font-size-xs);
	}

	.remote-queue-now {
		display: inline-block;
		margin-right: 6px;
		padding: 1px 6px;
		border-radius: 4px;
		background: var(--accent);
		color: var(--surface-0);
		font-size: var(--font-size-2xs);
		font-style: normal;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		vertical-align: 1px;
	}

	.remote-queue-chip:active {
		background: var(--surface-2);
	}

	.remote-queue-chip:disabled {
		opacity: 0.4;
	}

	.remote-queue-drag {
		width: 36px;
		height: 40px;
		display: grid;
		place-items: center;
		border-radius: 8px;
		background: transparent;
		color: var(--text-muted);
		touch-action: none;
		cursor: grab;
	}

	.remote-queue-drag:active {
		cursor: grabbing;
		background: var(--surface-1);
	}

	.remote-queue-drag:disabled {
		opacity: 0.3;
	}

	.remote-queue-drag svg {
		width: 18px;
		height: 18px;
	}

	.remote-queue-header-actions {
		display: flex;
		align-items: center;
		gap: 10px;
	}

	.remote-queue-count {
		color: var(--text-muted);
	}

	.remote-queue-clear {
		min-height: 36px;
		padding: 0 12px;
		border-radius: 8px;
		background: var(--surface-1);
		color: var(--text-primary);
		font-size: var(--font-size-xs);
	}

	.remote-queue-clear:active {
		background: var(--surface-2);
	}

	.remote-queue-clear:disabled {
		opacity: 0.4;
	}

	.remote-queue-undo {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		margin: 8px 0 12px;
		padding: 10px 14px;
		border-radius: 12px;
		background: color-mix(in srgb, var(--accent-soft) 70%, transparent);
		border: 1px solid var(--accent-line);
		font-size: var(--font-size-sm);
	}

	.remote-queue-undo-btn {
		padding: 6px 14px;
		border-radius: 999px;
		border: 1px solid var(--accent-line);
		background: var(--accent-strong);
		color: var(--bg-base);
		font-weight: var(--font-weight-semibold);
		font-size: var(--font-size-sm);
	}
</style>
