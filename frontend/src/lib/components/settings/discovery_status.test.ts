import { describe, expect, it } from 'vitest';
import type { DiscoveryStatus } from '$lib/api/client';
import {
	applyTrainingProgress,
	describeDiscoveryRun,
	describeDiscoveryUpgrade,
	discoveryLastTrainedAt,
	discoveryModelLabel,
	discoveryModelHeldBack,
	shouldContinueDiscoveryCompletionRefresh,
	shouldRefreshAfterTerminalDiscoveryProgress
} from './discovery_status';

function discoveryStatus(overrides: Partial<DiscoveryStatus> = {}): DiscoveryStatus {
	return {
		fallback_active: false,
		active_model: {
			id: 13,
			model_key: 'discovery-fusion-v2:13',
			family: 'discovery-fusion-v2',
			dimension: 96,
			status: 'ready',
			is_active: true,
			trained_at: '2026-05-11 13:06:25',
			config_json: null,
			metrics_json: null,
			created_at: '2026-05-11 13:00:00'
		},
		selected_engine: 'v2',
		selected_engine_family: 'discovery-fusion-v2',
		selected_engine_trainable: true,
		latest_run: null,
		coverage_ratio: 0.82,
		playable_tracks: 100,
		embedded_tracks: 82,
		neighbor_tracks: 82,
		clip_cache_tracks: 82,
		...overrides
	};
}

describe('discovery status display', () => {
	it('uses the latest completed training run time ahead of the active model time', () => {
		const status = discoveryStatus({
			latest_run: {
				id: 21,
				model_id: 21,
				stage: 'evaluate',
				status: 'completed',
				progress: 1,
				items_total: null,
				items_done: 0,
				started_at: '2026-05-31 09:00:00',
				finished_at: '2026-05-31 09:13:29',
				error_text: null
			}
		});

		expect(discoveryLastTrainedAt(status)).toBe('2026-05-31 09:13:29');
	});

	it('keeps the active model time while the latest run is not successful', () => {
		const status = discoveryStatus({
			latest_run: {
				id: 22,
				model_id: 22,
				stage: 'audio',
				status: 'failed',
				progress: 1,
				items_total: null,
				items_done: 0,
				started_at: '2026-05-31 10:00:00',
				finished_at: '2026-05-31 10:02:00',
				error_text: 'Audio setup failed'
			}
		});

		expect(discoveryLastTrainedAt(status)).toBe('2026-05-11 13:06:25');
	});

	it('does not show a V2 completed run as the V1 legacy training date', () => {
		const status = discoveryStatus({
			selected_engine: 'v1',
			selected_engine_family: 'discovery-fusion',
			selected_engine_trainable: false,
			latest_run: {
				id: 23,
				model_id: 23,
				stage: 'evaluate',
				status: 'completed',
				progress: 1,
				items_total: null,
				items_done: 0,
				started_at: '2026-05-31 11:00:00',
				finished_at: '2026-05-31 11:10:00',
				error_text: null
			}
		});

		expect(discoveryLastTrainedAt(status)).toBe('2026-05-11 13:06:25');
	});

	it('returns null when there is no completed run or active model date', () => {
		const status = discoveryStatus({ active_model: null });

		expect(discoveryLastTrainedAt(status)).toBeNull();
	});

	it('starts completion refreshes only for terminal discovery progress', () => {
		expect(
			shouldRefreshAfterTerminalDiscoveryProgress({
				type: 'training_progress',
				stage: 'evaluate',
				progress: 0.96
			})
		).toBe(true);
		expect(
			shouldRefreshAfterTerminalDiscoveryProgress({
				type: 'training_progress',
				stage: 'neighbors',
				progress: 0.96
			})
		).toBe(false);
		expect(
			shouldRefreshAfterTerminalDiscoveryProgress({
				type: 'training_progress',
				stage: 'evaluate',
				progress: 0.9
			})
		).toBe(false);
	});

	it('ignores malformed progress messages', () => {
		expect(
			shouldRefreshAfterTerminalDiscoveryProgress({
				type: 'training_progress',
				stage: 'evaluate'
			})
		).toBe(false);
	});

	it('continues completion refreshes only while the latest run is running and attempts remain', () => {
		const running = discoveryStatus({
			latest_run: {
				id: 24,
				model_id: 24,
				stage: 'evaluate',
				status: 'running',
				progress: 0.96,
				items_total: null,
				items_done: 0,
				started_at: '2026-05-31 12:00:00',
				finished_at: null,
				error_text: null
			}
		});
		const completed = discoveryStatus({
			latest_run: {
				...running.latest_run!,
				status: 'completed',
				progress: 1,
				finished_at: '2026-05-31 12:10:00'
			}
		});

		expect(shouldContinueDiscoveryCompletionRefresh(running, 1, 12)).toBe(true);
		expect(shouldContinueDiscoveryCompletionRefresh(running, 12, 12)).toBe(false);
		expect(shouldContinueDiscoveryCompletionRefresh(completed, 1, 12)).toBe(false);
		expect(shouldContinueDiscoveryCompletionRefresh(null, 1, 12)).toBe(false);
	});

	it('flags a completed run whose model did not replace the active one', () => {
		const status = discoveryStatus({
			latest_run: {
				id: 30,
				model_id: 30,
				stage: 'evaluate',
				status: 'completed',
				progress: 1,
				items_total: null,
				items_done: 0,
				started_at: '2026-10-01 09:00:00',
				finished_at: '2026-10-01 09:10:00',
				error_text: null
			}
		});

		expect(discoveryModelHeldBack(status)).toBe(true);
	});

	it('does not flag a run that became the active model', () => {
		const status = discoveryStatus({
			latest_run: {
				id: 13,
				model_id: 13,
				stage: 'evaluate',
				status: 'completed',
				progress: 1,
				items_total: null,
				items_done: 0,
				started_at: '2026-05-11 13:00:00',
				finished_at: '2026-05-11 13:06:25',
				error_text: null
			}
		});

		expect(discoveryModelHeldBack(status)).toBe(false);
	});

	it('does not flag failed runs or the V1 engine', () => {
		const failed = discoveryStatus({
			latest_run: {
				id: 31,
				model_id: 31,
				stage: 'audio',
				status: 'failed',
				progress: 1,
				items_total: null,
				items_done: 0,
				started_at: '2026-10-01 10:00:00',
				finished_at: '2026-10-01 10:02:00',
				error_text: 'Audio setup failed'
			}
		});
		expect(discoveryModelHeldBack(failed)).toBe(false);
		expect(discoveryModelHeldBack(discoveryStatus({ selected_engine: 'v1' }))).toBe(false);
	});
});

describe('training progress merge', () => {
	const terminalRun = {
		id: 30,
		model_id: 30,
		stage: 'evaluate',
		status: 'completed',
		progress: 1,
		items_total: 100,
		items_done: 100,
		started_at: '2026-07-26 09:00:00',
		finished_at: '2026-07-26 09:10:00',
		error_text: null
	};

	it('marks the run running so the Stop button appears', () => {
		// Regression: the status read right after starting a run came from a 30s
		// cache still holding the previous run's terminal status, and the progress
		// merge never rewrote `status`. The bar advanced with no Stop button.
		const merged = applyTrainingProgress(discoveryStatus({ latest_run: terminalRun }), {
			progress: 0.2,
			stage: 'behavioral',
			tracks_done: 20,
			tracks_total: 100
		});

		expect(merged?.latest_run?.status).toBe('running');
		expect(merged?.latest_run?.progress).toBe(0.2);
		expect(merged?.latest_run?.stage).toBe('behavioral');
		expect(merged?.latest_run?.items_done).toBe(20);
	});

	it('keeps existing fields when the event omits them', () => {
		const merged = applyTrainingProgress(
			discoveryStatus({ latest_run: { ...terminalRun, progress: 0.4, stage: 'audio' } }),
			{}
		);

		expect(merged?.latest_run?.progress).toBe(0.4);
		expect(merged?.latest_run?.stage).toBe('audio');
		expect(merged?.latest_run?.status).toBe('running');
	});

	it('is a no-op when there is no run to merge into', () => {
		expect(applyTrainingProgress(discoveryStatus({ latest_run: null }), { progress: 0.5 })
			?.latest_run).toBeNull();
		expect(applyTrainingProgress(null, { progress: 0.5 })).toBeNull();
	});
});

describe('trainer panel wording', () => {
	const run = (status: string, extra: Partial<NonNullable<DiscoveryStatus['latest_run']>> = {}) => ({
		id: 1,
		model_id: null,
		stage: 'neighbors',
		status,
		progress: 0.42,
		items_total: null,
		items_done: 0,
		started_at: '2026-10-10 10:00:00',
		finished_at: null,
		error_text: null,
		...extra,
	});

	it('describes the latest run in words', () => {
		expect(describeDiscoveryRun(null)).toBe('Never run');
		expect(describeDiscoveryRun(run('running'))).toBe('Computing neighbors - 42%');
		expect(describeDiscoveryRun(run('running', { stage: 'corpus', progress: 0.08 }))).toBe(
			'Reading your library - 8%'
		);
		expect(describeDiscoveryRun(run('running', { stage: 'saving', progress: 0.975 }))).toBe(
			'Saving results - 98%'
		);
		expect(describeDiscoveryRun(run('completed'))).toBe('Finished');
		expect(describeDiscoveryRun(run('cancelled'))).toBe('Stopped');
		expect(describeDiscoveryRun(run('failed', { error_text: 'interrupted by server restart' }))).toBe(
			'Interrupted when NOOR closed'
		);
		expect(describeDiscoveryRun(run('failed', { error_text: 'boom' }))).toBe('Failed');
	});

	it('only mentions the upgrade retrain while one is due or running', () => {
		expect(describeDiscoveryUpgrade(null)).toBeNull();
		expect(describeDiscoveryUpgrade({ pending: false, running: false, trainer_version: 3 })).toBeNull();
		expect(describeDiscoveryUpgrade({ pending: true, running: true, trainer_version: 3 })).toMatch(
			/low priority/
		);
		expect(describeDiscoveryUpgrade({ pending: true, running: false, trainer_version: 3 })).toMatch(
			/Full retrain/
		);
	});

	it('names the active model by id and flags it while an upgrade replaces it', () => {
		const model = discoveryStatus().active_model;
		expect(discoveryModelLabel(null)).toBe('None yet');
		expect(discoveryModelLabel(model)).toBe('Model 13');
		expect(discoveryModelLabel(model, { pending: false, running: false, trainer_version: 3 })).toBe(
			'Model 13'
		);
		expect(discoveryModelLabel(model, { pending: true, running: true, trainer_version: 3 })).toBe(
			'Model 13 (outdated)'
		);
	});
});
