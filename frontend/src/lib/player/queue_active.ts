import type { QueueItem, Track } from '$lib/api/client';

function rowMatchesTrack(item: QueueItem, track: Track): boolean {
	return item.track.id === track.id;
}

function rowMatchesPendingCurrent(item: QueueItem): boolean {
	return item.is_pending === true || item.track.id === 0;
}

export function isQueueItemActive(
	item: QueueItem,
	currentTrack: Track | null | undefined,
	currentQueueItemId: number | null | undefined,
	queue: QueueItem[],
): boolean {
	if (currentQueueItemId == null) {
		return currentTrack != null && rowMatchesTrack(item, currentTrack);
	}

	const anchor = queue.find((row) => row.id === currentQueueItemId);
	const anchorMatchesCurrent =
		anchor != null &&
		(currentTrack != null ? rowMatchesTrack(anchor, currentTrack) : rowMatchesPendingCurrent(anchor));

	if (anchorMatchesCurrent) {
		return item.id === currentQueueItemId;
	}

	return currentTrack != null && rowMatchesTrack(item, currentTrack);
}
