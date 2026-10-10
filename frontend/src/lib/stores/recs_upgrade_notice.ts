import { api } from '$lib/api/client';
import { dismissToast, showToast } from '$lib/stores/toast';

const DISMISSED_KEY_PREFIX = 'noor.recs_upgrade_notice.v';

function dismissedKey(version: number): string {
	return `${DISMISSED_KEY_PREFIX}${version}`;
}

function isDismissed(version: number): boolean {
	try {
		return localStorage.getItem(dismissedKey(version)) === '1';
	} catch {
		return false;
	}
}

function rememberDismissed(version: number): void {
	try {
		localStorage.setItem(dismissedKey(version), '1');
	} catch {
		// Storage full or blocked: the notice may show again next launch.
	}
}

/**
 * One notice per trainer version when an update needs recommendations to be
 * relearned. The retrain itself runs on its own at low priority; "Run now"
 * (offered only while it is still waiting for idle time) starts it at full
 * speed instead.
 */
export async function maybeShowRecsUpgradeNotice(): Promise<void> {
	let status;
	try {
		status = await api.getDiscoveryUpgrade();
	} catch {
		return;
	}
	if (!status.pending || isDismissed(status.trainer_version)) return;

	const version = status.trainer_version;
	const message = status.running
		? 'Recommendations were upgraded. NOOR is relearning your taste in the background at low priority; suggestions get better when it finishes.'
		: 'Recommendations were upgraded. NOOR will relearn your taste in the background when your computer is idle (about 15 minutes), or you can start it now.';
	const id: number = showToast(message, 'info', Infinity, [
		...(status.running
			? []
			: [
					{
						label: 'Run now',
						onClick: () => {
							rememberDismissed(version);
							dismissToast(id);
							void api.startDiscoveryTraining('full', true);
						},
					},
				]),
		{
			label: 'Got it',
			onClick: () => {
				rememberDismissed(version);
				dismissToast(id);
			},
		},
	]);
}
