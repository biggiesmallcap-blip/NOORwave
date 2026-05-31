import { describe, expect, it } from 'vitest';
import type { DiscoveryStatus } from '$lib/api/client';
import {
	discoveryLastTrainedAt,
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
});
