import type { DiscoveryStatus, DiscoveryTrainingRun } from '$lib/api/client';

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

/**
 * True when the latest run finished but its model did not replace the active
 * one: the activation gate held it back because it did not beat the active
 * model on the same held-out listening.
 */
export function discoveryModelHeldBack(status: DiscoveryStatus | null): boolean {
	const run = status?.latest_run;
	const active = status?.active_model;
	return (
		status?.selected_engine === 'v2' &&
		run?.status === 'completed' &&
		run.model_id != null &&
		active != null &&
		run.model_id !== active.id
	);
}

const STAGE_LABELS: Record<string, string> = {
	behavioral: 'Learning listening patterns',
	audio: 'Processing audio features',
	fusion: 'Blending features',
	neighbors: 'Computing neighbors',
	in_degree: 'Ranking connections',
	evaluate: 'Evaluating',
};

export function discoveryStageLabel(stage: string | undefined): string {
	return (stage && STAGE_LABELS[stage]) || 'Computing';
}

/** The latest run in words: what it is doing now, or how it ended. */
export function describeDiscoveryRun(run: DiscoveryTrainingRun | null | undefined): string {
	if (!run) return 'Never run';
	switch (run.status) {
		case 'running':
			return `${discoveryStageLabel(run.stage)} - ${Math.round((run.progress ?? 0) * 100)}%`;
		case 'completed':
			return 'Finished';
		case 'cancelled':
			return 'Stopped';
		case 'failed':
			return run.error_text === 'interrupted by server restart'
				? 'Interrupted when NOOR closed'
				: 'Failed';
		default:
			return run.status;
	}
}

export interface DiscoveryUpgrade {
	pending: boolean;
	running: boolean;
	trainer_version: number;
}

/**
 * What the one-time upgrade retrain is doing, or null when there is none. It
 * runs on its own at low priority; Full retrain starts it at full speed.
 */
export function describeDiscoveryUpgrade(upgrade: DiscoveryUpgrade | null): string | null {
	if (!upgrade?.pending) return null;
	return upgrade.running
		? 'Relearning in the background at low priority'
		: 'Waiting for idle time (Full retrain runs it now)';
}

/** "Model 21 (trainer v3)" rather than the internal model key. */
export function discoveryModelLabel(model: DiscoveryStatus['active_model']): string {
	if (!model) return 'Fallback only';
	let version: number | null = null;
	try {
		const parsed = model.config_json ? JSON.parse(model.config_json) : null;
		if (typeof parsed?.trainer_config_version === 'number') version = parsed.trainer_config_version;
	} catch {
		// Older models may carry no or malformed config; the id still identifies them.
	}
	return version === null ? `Model ${model.id}` : `Model ${model.id} (trainer v${version})`;
}
