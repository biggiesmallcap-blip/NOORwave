import type { DiscoveryStatus } from '$lib/api/client';

export function discoveryLastTrainedAt(status: DiscoveryStatus | null): string | null {
	const completedRunAt =
		status?.selected_engine === 'v2' && status.latest_run?.status === 'completed'
			? status.latest_run.finished_at
			: null;
	return completedRunAt ?? status?.active_model?.trained_at ?? null;
}

export function shouldRefreshAfterTerminalDiscoveryProgress(message: {
	type?: string;
	stage?: string;
	progress?: number;
}): boolean {
	return (
		message.type === 'training_progress' &&
		message.stage === 'evaluate' &&
		typeof message.progress === 'number' &&
		message.progress >= 0.95
	);
}

/**
 * Fold a `training_progress` websocket event into the cached status.
 *
 * Also forces `status` to 'running'. A progress event is only emitted by a live
 * run, so its arrival is proof the run is active - and the Stop button renders
 * on `latest_run.status === 'running'`. Without this the button stayed hidden
 * for the whole run: the status read right after starting came from a 30s cache
 * still holding the PREVIOUS run's terminal status, and nothing else ever
 * rewrote it, so the progress bar advanced with no way to stop it.
 */
export function applyTrainingProgress(
	status: DiscoveryStatus | null,
	message: {
		progress?: number;
		stage?: string;
		tracks_done?: number;
		tracks_total?: number;
	}
): DiscoveryStatus | null {
	if (!status?.latest_run) return status;
	return {
		...status,
		latest_run: {
			...status.latest_run,
			status: 'running',
			progress:
				typeof message.progress === 'number' ? message.progress : status.latest_run.progress,
			stage: typeof message.stage === 'string' ? message.stage : status.latest_run.stage,
			items_done:
				typeof message.tracks_done === 'number'
					? message.tracks_done
					: status.latest_run.items_done,
			items_total:
				typeof message.tracks_total === 'number'
					? message.tracks_total
					: status.latest_run.items_total,
		},
	};
}

export function shouldContinueDiscoveryCompletionRefresh(
	status: DiscoveryStatus | null,
	attempts: number,
	maxAttempts: number
): boolean {
	return status?.latest_run?.status === 'running' && attempts < maxAttempts;
}
