import type { api } from '$lib/api/client';

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
	return {
		dispose() { disposed = true; },
		async refresh(force = false): Promise<CockpitSnapshot | null> {
			if (disposed || busy || (!force && now() < retryAt)) return null;
			busy = true;
			try {
				const [enabled, policy, status] = await Promise.allSettled([
					client.getDjEnabled(), client.getDjPolicy(), client.getDjStatus(),
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
