import type {
	AudioDspFeatures,
	DiscoveryStatus,
	PlaybackRuntimeInfo,
	QueueItem,
	Track
} from '$lib/api/client';
import { harmonicCompat } from '$lib/utils/camelot';
import { parseReason } from '$lib/utils/reason';

export type AutomixVerdict = 'good' | 'okay' | 'clash' | 'pending' | 'unknown';
export type AutomixHealthStatus = 'ready' | 'degraded' | 'blocked';
export type AutomixFeatureLookup = (trackId: number) => AudioDspFeatures | null | undefined;

export interface AutomixForecastRow {
	item: QueueItem;
	previousTrack: Track | null;
	previousFeatures: AudioDspFeatures | null | undefined;
	nextFeatures: AudioDspFeatures | null | undefined;
	verdict: AutomixVerdict;
	keyLabel: string | null;
	bpmDelta: number | null;
	bpmDeltaLabel: string | null;
	energyDeltaLabel: string | null;
	sourceLabel: string;
	selectionReasonLabel: string | null;
	missing: string[];
	isExternalPending: boolean;
}

export interface AutomixForecastCounts {
	good: number;
	okay: number;
	clash: number;
	pending: number;
	unknown: number;
	externalPending: number;
}

export interface AutomixHealthInput {
	automixEnabled: boolean;
	currentTrack: Track | null;
	currentFeatures: AudioDspFeatures | null;
	upcomingCount: number;
	pendingCount: number;
	runtimeAvailable: boolean;
	runtime?: PlaybackRuntimeInfo | null;
	discoveryStatus: DiscoveryStatus | null;
}

export interface AutomixHealth {
	status: AutomixHealthStatus;
	label: string;
	reasons: string[];
}

export function formatFeatureSummary(features: AudioDspFeatures | null | undefined): string {
	if (!features) return 'DSP pending';
	const parts = [
		features.camelot_key ?? features.key_signature,
		features.bpm ? `${Math.round(features.bpm)} BPM` : null,
		features.energy != null ? `${Math.round(features.energy * 100)}% energy` : null
	].filter(Boolean);
	return parts.join(' / ') || 'DSP pending';
}

export function bpmDeltaLabel(delta: number | null): string | null {
	if (delta === null) return null;
	const sign = delta > 0 ? '+' : '';
	return `${sign}${delta.toFixed(1)} BPM`;
}

export function energyDeltaLabel(
	previous: AudioDspFeatures | null | undefined,
	next: AudioDspFeatures | null | undefined
): string | null {
	if (previous?.energy == null || next?.energy == null) return null;
	const delta = Math.round((next.energy - previous.energy) * 100);
	const sign = delta > 0 ? '+' : '';
	return `${sign}${delta}% energy`;
}

export function queueSourceLabel(source: string): string {
	const normalized = source.trim().toLowerCase();
	if (normalized === 'automix-new') return 'External';
	if (normalized.startsWith('automix')) return 'Automix';
	if (normalized.includes('radio')) return 'Radio';
	if (normalized.includes('manual') || normalized.includes('queue')) return 'Manual';
	return source || 'Queued';
}

export function selectionReasonLabel(
	reason: string | null | undefined,
	source: string
): string | null {
	const parsed = parseReason(reason);
	if (parsed?.prefix) return parsed.prefix;
	if (source.trim().toLowerCase().startsWith('automix')) {
		return 'Reason not recorded for this row';
	}
	return null;
}

function missingFeatureLabels(
	previous: AudioDspFeatures | null | undefined,
	next: AudioDspFeatures | null | undefined
): string[] {
	const missing: string[] = [];
	if (!previous) missing.push('previous DSP');
	if (!next) missing.push('next DSP');
	return missing;
}

export function buildForecastRows(input: {
	currentTrack: Track | null;
	currentFeatures: AudioDspFeatures | null;
	upcoming: QueueItem[];
	featuresFor: AutomixFeatureLookup;
}): AutomixForecastRow[] {
	return input.upcoming.map((item, index) => {
		const previousTrack = index === 0 ? input.currentTrack : input.upcoming[index - 1].track;
		const previousFeatures =
			index === 0 && input.currentTrack?.id === previousTrack?.id
				? input.currentFeatures
				: input.featuresFor(previousTrack?.id ?? -1);
		const nextFeatures = input.featuresFor(item.track.id);
		const compat = harmonicCompat(previousFeatures, nextFeatures);
		const missing = missingFeatureLabels(previousFeatures, nextFeatures);
		const isExternalPending = item.source === 'automix-new' || item.is_pending === true;
		const verdict: AutomixVerdict =
			isExternalPending || missing.length > 0 ? 'pending' : (compat?.level ?? 'unknown');

		return {
			item,
			previousTrack,
			previousFeatures,
			nextFeatures,
			verdict,
			keyLabel: compat?.keyLabel ?? null,
			bpmDelta: compat?.bpmDelta ?? null,
			bpmDeltaLabel: bpmDeltaLabel(compat?.bpmDelta ?? null),
			energyDeltaLabel: energyDeltaLabel(previousFeatures, nextFeatures),
			sourceLabel: queueSourceLabel(item.source),
			selectionReasonLabel: selectionReasonLabel(item.reason, item.source),
			missing,
			isExternalPending
		};
	});
}

export function countForecastRows(rows: AutomixForecastRow[]): AutomixForecastCounts {
	return rows.reduce<AutomixForecastCounts>(
		(counts, row) => {
			counts[row.verdict] += 1;
			if (row.isExternalPending) counts.externalPending += 1;
			return counts;
		},
		{ good: 0, okay: 0, clash: 0, pending: 0, unknown: 0, externalPending: 0 }
	);
}

export function automixHealth(input: AutomixHealthInput): AutomixHealth {
	const reasons: string[] = [];

	if (!input.automixEnabled) reasons.push('Automix is off');
	if (!input.currentTrack) reasons.push('No active seed');
	if (input.currentTrack && !input.currentFeatures) reasons.push('Seed DSP is missing');
	if (input.upcomingCount === 0) reasons.push('Queue is empty');
	if (!input.runtimeAvailable) reasons.push('Playback runtime is offline');
	if (input.pendingCount > 0) {
		reasons.push(`${input.pendingCount} external row${input.pendingCount === 1 ? '' : 's'} pending`);
	}
	if (input.discoveryStatus && input.discoveryStatus.coverage_ratio < 0.5) {
		reasons.push('Discovery coverage is low');
	}
	if (!input.discoveryStatus) reasons.push('Discovery status is unknown');
	if (input.runtime?.last_error) reasons.push(input.runtime.last_error);

	if (!input.automixEnabled || !input.currentTrack) {
		return { status: 'blocked', label: 'Blocked', reasons };
	}

	if (reasons.length > 0) {
		return { status: 'degraded', label: 'Degraded', reasons };
	}

	return { status: 'ready', label: 'Ready', reasons: ['Automix has seed, queue, DSP, and runtime data'] };
}

/**
 * Drop the cached features entry for a track. Returned boolean lets the caller
 * decide whether to refetch + bump a reactivity counter; an unknown trackId is
 * a no-op (the WS event may fire for a track the cockpit isn't currently
 * rendering features for).
 */
export function invalidateCacheForTrack(
	cache: Map<number, AudioDspFeatures | null>,
	trackId: number
): boolean {
	if (!cache.has(trackId)) return false;
	cache.delete(trackId);
	return true;
}
