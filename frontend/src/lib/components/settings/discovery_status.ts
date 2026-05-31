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

export function shouldContinueDiscoveryCompletionRefresh(
	status: DiscoveryStatus | null,
	attempts: number,
	maxAttempts: number
): boolean {
	return status?.latest_run?.status === 'running' && attempts < maxAttempts;
}
