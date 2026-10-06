import type { api } from '$lib/api/client';
import type { DjStatusResponse } from '$lib/api/client';

export function cockpitPollInterval(status: DjStatusResponse | null, positionMs: number, playing: boolean, visible: boolean): number {
	if (!visible || !playing || !status?.enabled) return 2000;
	if (status.active_transition) return 500;
	const starts = [status.runtime_planned_start_ms ?? status.planned_start_ms,
		status.drop_preview?.status === 'armed' ? status.drop_preview.planned_fire_ms : undefined];
	return starts.some((start) => start != null && start - positionMs <= 4000 && start - positionMs >= -1000) ? 500 : 2000;
}

export function newlyConfirmedCut(previous: DjStatusResponse | null, next: DjStatusResponse): string | null {
	if (!previous) return null;
	const confirmed = (event: DjStatusResponse['recent_timing_events'][number]) =>
		event.actual_start_ms != null && ['fired', 'late'].includes(event.timing_status ?? '');
	const prior = new Set((previous.recent_timing_events ?? []).filter(confirmed).map((event) => event.event_id));
	const cut = (next.recent_timing_events ?? []).find((event) => confirmed(event) && !prior.has(event.event_id)
		&& event.runtime_rendered_dj_mixer && event.runtime_renderer_status === 'rendered_handoff'
		&& (event.renderer_template ?? event.planned_template) === 'SlamCut');
	return cut ? `Cut fired · ${cut.from_title ?? 'Outgoing track'} → ${cut.to_title ?? 'Incoming track'}` : null;
}

type Client = Pick<typeof api, 'getDjEnabled' | 'getDjPolicy' | 'getDjStatus'>;
export type CockpitSnapshot = {
	enabled?: Awaited<ReturnType<Client['getDjEnabled']>>;
	policy?: Awaited<ReturnType<Client['getDjPolicy']>>;
	status?: Awaited<ReturnType<Client['getDjStatus']>>;
	error: string;
};

// One request group at a time; partial success remains useful during a retry.
export function createCockpitRefresh(client: Client, now = Date.now) {
	let busy = false;
	let disposed = false;
	let failures = 0;
	let retryAt = 0;
	let controlsAt = 0;
	return {
		dispose() { disposed = true; },
		async refresh(force = false): Promise<CockpitSnapshot | null> {
			if (disposed || busy || (!force && now() < retryAt)) return null;
			busy = true;
			try {
				const requestControls = force || now() >= controlsAt;
				if (requestControls) controlsAt = now() + 2000;
				const [enabled, policy, status] = await Promise.allSettled([
					requestControls ? client.getDjEnabled() : Promise.resolve(undefined),
					requestControls ? client.getDjPolicy() : Promise.resolve(undefined), client.getDjStatus(),
				]);
				if (disposed) return null;
				const errors = [enabled, policy, status].flatMap((result, index) =>
					result.status === 'rejected' ? [`${['DJ controls', 'Mix policy', 'Transition status'][index]}: ${result.reason instanceof Error ? result.reason.message : 'Request failed'}`] : []);
				failures = errors.length ? failures + 1 : 0;
				retryAt = errors.length ? now() + Math.min(30_000, 2_000 * 2 ** Math.min(failures - 1, 4)) : 0;
				return {
					enabled: enabled.status === 'fulfilled' ? enabled.value : undefined,
					policy: policy.status === 'fulfilled' ? policy.value : undefined,
					status: status.status === 'fulfilled' ? status.value : undefined,
					error: errors.join(' · '),
				};
			} finally { busy = false; }
		},
	};
}
