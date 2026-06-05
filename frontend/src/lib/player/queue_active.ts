import type { QueueItem, Track } from '$lib/api/client';

function rowMatchesTrack(item: QueueItem, track: Track): boolean {
	return item.track.id === track.id;
}

function rowMatchesPendingCurrent(item: QueueItem): boolean {
	return item.is_pending === true || item.track.id === 0;
}

function currentQueueAnchor(
	queue: QueueItem[],
	currentTrack: Track | null | undefined,
	currentQueueItemId: number | null | undefined,
): QueueItem | null {
	if (currentQueueItemId == null) {
		return currentTrack != null
			? (queue.find((row) => rowMatchesTrack(row, currentTrack)) ?? null)
			: null;
	}

	const anchor = queue.find((row) => row.id === currentQueueItemId);
	const anchorMatchesCurrent =
		anchor != null &&
		(currentTrack != null ? rowMatchesTrack(anchor, currentTrack) : rowMatchesPendingCurrent(anchor));

	if (anchorMatchesCurrent) {
		return anchor;
	}

	return currentTrack != null
		? (queue.find((row) => rowMatchesTrack(row, currentTrack)) ?? null)
		: null;
}

export function currentQueueAnchorPosition(
	queue: QueueItem[],
	currentTrack: Track | null | undefined,
	currentQueueItemId: number | null | undefined,
): number | null {
	return currentQueueAnchor(queue, currentTrack, currentQueueItemId)?.position ?? null;
}

export function isQueueItemActive(
	item: QueueItem,
	currentTrack: Track | null | undefined,
	currentQueueItemId: number | null | undefined,
	queue: QueueItem[],
): boolean {
	return item.id === currentQueueAnchor(queue, currentTrack, currentQueueItemId)?.id;
}
