import { describe, expect, it } from 'vitest';
import type { AudioDspFeatures, DiscoveryStatus, QueueItem, Track } from '$lib/api/client';
import {
	automixHealth,
	buildForecastRows,
	countForecastRows,
	energyDeltaLabel,
	formatFeatureSummary,
	type AutomixFeatureLookup
} from './automix_diagnostics';

function track(id: number, title = `Track ${id}`): Track {
	return {
		id,
		title,
		artist_id: id,
		artist_name: `Artist ${id}`,
		album_id: id,
		album_title: `Album ${id}`,
		disc_number: null,
		track_number: null,
		duration_ms: 180000,
		isrc: null,
		tidal_id: null,
		best_quality: 'LOSSLESS',
		best_source: 'local',
		fidelity_score: 1,
		is_favorite: false,
		play_count: 0,
		last_played_at: null,
		date_added: null,
		source: 'local',
		artwork_url: null
	};
}

function queueItem(id: number, source = 'automix', isPending = false): QueueItem {
	return {
		id,
		position: id,
		source,
		track: track(id),
		is_pending: isPending,
		reason: null
	};
}

function features(trackId: number, overrides: Partial<AudioDspFeatures> = {}): AudioDspFeatures {
	return {
		track_id: trackId,
		bpm: 120,
		key_signature: 'Am',
		camelot_key: '8A',
		loudness_lufs: -11,
		energy: 0.65,
		danceability: 0.7,
		beat_strength: 0.7,
		spectral_centroid: 2400,
		stereo_width: 0.6,
		is_instrumental: false,
		analysis_source: 'test',
		analysis_offset_ms: 0,
		samples_analyzed: 120000,
		analyzed_at: '2026-05-13T00:00:00Z',
		analysis_version: 'test',
		...overrides
	};
}

function discoveryStatus(overrides: Partial<DiscoveryStatus> = {}): DiscoveryStatus {
	return {
		fallback_active: false,
		active_model: null,
		selected_engine: 'v2',
		selected_engine_family: 'audio',
		selected_engine_trainable: true,
		latest_run: null,
		coverage_ratio: 0.82,
		playable_tracks: 1000,
		embedded_tracks: 820,
		neighbor_tracks: 900,
		clip_cache_tracks: 750,
		...overrides
	};
}

describe('automix diagnostics', () => {
	it('marks missing DSP as pending instead of a clash', () => {
		const lookup: AutomixFeatureLookup = (id) => (id === 1 ? features(1) : undefined);
		const rows = buildForecastRows({
			currentTrack: track(1),
			currentFeatures: features(1),
			upcoming: [queueItem(2)],
			featuresFor: lookup
		});

		expect(rows[0].verdict).toBe('pending');
		expect(rows[0].missing).toContain('next DSP');
	});

	it('marks compatible key and BPM transitions as good', () => {
		const lookup: AutomixFeatureLookup = (id) =>
			id === 2 ? features(2, { bpm: 124, camelot_key: '8A' }) : undefined;
		const rows = buildForecastRows({
			currentTrack: track(1),
			currentFeatures: features(1, { bpm: 120, camelot_key: '8A' }),
			upcoming: [queueItem(2)],
			featuresFor: lookup
		});

		expect(rows[0].verdict).toBe('good');
		expect(rows[0].bpmDelta).toBe(4);
		expect(rows[0].keyLabel).toBe('same key');
	});

	it('counts pending external rows separately', () => {
		const rows = buildForecastRows({
			currentTrack: track(1),
			currentFeatures: features(1),
			upcoming: [queueItem(2, 'automix-new', true), queueItem(3, 'manual')],
			featuresFor: () => undefined
		});

		expect(countForecastRows(rows)).toMatchObject({
			pending: 2,
			externalPending: 1,
			good: 0,
			clash: 0
		});
	});

	it('degrades health when seed DSP is missing or queue is empty', () => {
		expect(
			automixHealth({
				automixEnabled: true,
				currentTrack: track(1),
				currentFeatures: null,
				upcomingCount: 3,
				pendingCount: 0,
				runtimeAvailable: true,
				discoveryStatus: discoveryStatus()
			})
		).toMatchObject({ status: 'degraded' });

		expect(
			automixHealth({
				automixEnabled: true,
				currentTrack: track(1),
				currentFeatures: features(1),
				upcomingCount: 0,
				pendingCount: 0,
				runtimeAvailable: true,
				discoveryStatus: discoveryStatus()
			}).reasons
		).toContain('Queue is empty');
	});

	it('formats feature and energy labels', () => {
		expect(formatFeatureSummary(features(1, { bpm: 119.6, camelot_key: '7B', energy: 0.51 }))).toBe(
			'7B / 120 BPM / 51% energy'
		);
		expect(formatFeatureSummary(null)).toBe('DSP pending');
		expect(energyDeltaLabel(features(1, { energy: 0.4 }), features(2, { energy: 0.72 }))).toBe(
			'+32% energy'
		);
	});
});
