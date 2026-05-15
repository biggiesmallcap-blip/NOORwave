<script lang="ts">
	import type { QueueItem } from '$lib/api/client';
	import { playTrackNow, removeTrackFromQueue } from '$lib/stores/player';
	import { formatTrackDuration } from '$lib/utils/format';

	let { queue }: { queue: QueueItem[] } = $props();

	function canPlay(item: QueueItem): boolean {
		return item.is_pending !== true && item.track.id > 0;
	}
</script>

<section class="remote-queue" aria-label="Up next">
	<header>
		<h2>Up next</h2>
		<span>{queue.length}</span>
	</header>

	{#if queue.length === 0}
		<p class="remote-empty">Nothing is lined up.</p>
	{:else}
		<div class="remote-queue-list">
			{#each queue.slice(0, 20) as item (item.id)}
				<div class="remote-queue-row" class:pending={item.is_pending}>
					<button
						type="button"
						disabled={!canPlay(item)}
						aria-label="Play queued track"
						onclick={() => void playTrackNow(item.track.id)}
					>
						{#if item.track.artwork_url}
							<img src={item.track.artwork_url} alt="" />
						{:else}
							<span aria-hidden="true">NOOR</span>
						{/if}
						<span class="remote-queue-copy">
							<strong>{item.track.title}</strong>
							<small>{item.track.artist_name ?? 'Unknown artist'}</small>
						</span>
						<small>{formatTrackDuration(item.track.duration_ms)}</small>
					</button>
					<button
						class="remote-queue-remove"
						type="button"
						disabled={item.is_pending === true}
						aria-label="Remove from queue"
						onclick={() => void removeTrackFromQueue(item.id)}
					>
						Remove
					</button>
				</div>
			{/each}
		</div>
	{/if}
</section>

<style>
	.remote-queue {
		display: grid;
		gap: 12px;
	}

	.remote-queue header,
	.remote-queue-row,
	.remote-queue-row button:first-child {
		display: flex;
		align-items: center;
		gap: 10px;
	}

	.remote-queue header {
		justify-content: space-between;
	}

	.remote-queue h2,
	.remote-empty {
		margin: 0;
	}

	.remote-queue-list {
		display: grid;
		gap: 8px;
	}

	.remote-queue-row {
		min-height: 56px;
	}

	.remote-queue-row button:first-child {
		min-width: 0;
		flex: 1;
		text-align: left;
	}

	.remote-queue-row img,
	.remote-queue-row button:first-child > span:first-child {
		width: 44px;
		height: 44px;
		border-radius: 6px;
		object-fit: cover;
	}

	.remote-queue-copy {
		min-width: 0;
		display: grid;
	}

	.remote-queue-copy strong,
	.remote-queue-copy small {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-queue-remove {
		min-height: 44px;
	}
</style>
