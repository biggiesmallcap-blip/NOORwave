import { beforeEach, describe, expect, it, vi } from 'vitest';

const getDiscoveryUpgrade = vi.fn();
const startDiscoveryTraining = vi.fn();
const showToast = vi.fn();
const dismissToast = vi.fn();

vi.mock('$lib/api/client', () => ({
	api: {
		getDiscoveryUpgrade: () => getDiscoveryUpgrade(),
		startDiscoveryTraining: (...args: unknown[]) => startDiscoveryTraining(...args),
	},
}));
vi.mock('$lib/stores/toast', () => ({
	showToast: (...args: unknown[]) => showToast(...args),
	dismissToast: (...args: unknown[]) => dismissToast(...args),
}));

import { maybeShowRecsUpgradeNotice } from './recs_upgrade_notice';

type Action = { label: string; onClick: () => void };
const actions = (): Action[] => showToast.mock.calls[0][3] as Action[];

describe('recommendations upgrade notice', () => {
	beforeEach(() => {
		const store = new Map<string, string>();
		vi.stubGlobal('localStorage', {
			getItem: (key: string) => store.get(key) ?? null,
			setItem: (key: string, value: string) => void store.set(key, value),
			removeItem: (key: string) => void store.delete(key),
			clear: () => store.clear(),
		});
		for (const mock of [getDiscoveryUpgrade, startDiscoveryTraining, showToast, dismissToast]) {
			mock.mockReset();
		}
		showToast.mockReturnValue(7);
	});

	it('stays quiet when nothing is pending', async () => {
		getDiscoveryUpgrade.mockResolvedValue({ pending: false, running: false, trainer_version: 3 });
		await maybeShowRecsUpgradeNotice();
		expect(showToast).not.toHaveBeenCalled();
	});

	it('offers Run now while waiting, and remembers Got it per version', async () => {
		getDiscoveryUpgrade.mockResolvedValue({ pending: true, running: false, trainer_version: 3 });
		await maybeShowRecsUpgradeNotice();
		expect(actions().map((a) => a.label)).toEqual(['Run now', 'Got it']);

		actions()[1].onClick();
		expect(dismissToast).toHaveBeenCalledWith(7);
		showToast.mockClear();
		await maybeShowRecsUpgradeNotice();
		expect(showToast).not.toHaveBeenCalled();
	});

	it('Run now starts a full retrain', async () => {
		getDiscoveryUpgrade.mockResolvedValue({ pending: true, running: false, trainer_version: 3 });
		await maybeShowRecsUpgradeNotice();
		actions()[0].onClick();
		expect(startDiscoveryTraining).toHaveBeenCalledWith('full', true);
	});

	it('only says what is happening while the retrain runs', async () => {
		getDiscoveryUpgrade.mockResolvedValue({ pending: true, running: true, trainer_version: 3 });
		await maybeShowRecsUpgradeNotice();
		expect(actions().map((a) => a.label)).toEqual(['Got it']);
	});
});
