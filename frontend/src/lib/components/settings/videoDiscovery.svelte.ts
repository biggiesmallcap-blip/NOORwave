import { api, type VideoDiscoverySetting, type VideoDiscoveryStatus } from '$lib/api/client';

export const VIDEO_DISCOVERY_OPTIONS: { value: VideoDiscoverySetting; label: string; hint: string }[] = [
	{ value: 'full', label: 'Full', hint: 'Finds music videos in the background, faster while nothing is playing.' },
	{ value: 'limited', label: 'Limited', hint: 'Finds music videos slowly, with about a tenth of the requests.' },
	{ value: 'off', label: 'Off', hint: 'No background search. Video radio only looks up the station you start.' },
];

export class VideoDiscoverySettings {
	setting = $state<VideoDiscoverySetting>('full');
	status = $state<VideoDiscoveryStatus | null>(null);
	known = $state(false);
	busy = $state(false);
	error = $state('');

	async load() {
		if (this.busy) return;
		this.busy = true; this.error = '';
		try {
			this.setting = (await api.getVideoDiscoverySettings()).setting;
			this.known = true;
		} catch {
			this.known = false; this.error = 'Could not read the video discovery setting.';
		} finally { this.busy = false; }
		// Progress is a nicety; a failed read leaves the setting usable.
		try { this.status = await api.getVideoDiscoveryStatus(); } catch { this.status = null; }
	}

	async save(next: VideoDiscoverySetting) {
		if (!this.known || this.busy || next === this.setting) return;
		const previous = this.setting;
		this.setting = next; this.busy = true; this.error = '';
		try { this.setting = (await api.setVideoDiscoverySettings(next)).setting; }
		catch { this.setting = previous; this.error = 'Could not save the video discovery setting.'; }
		finally { this.busy = false; }
	}
}
