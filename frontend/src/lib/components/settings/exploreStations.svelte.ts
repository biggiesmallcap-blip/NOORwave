import { api, type VideoStationScene } from '$lib/api/client';

/** Explore stations on the Videos > Stations tab: a master switch and one
 *  switch per scene. A scene that is off is not built at all. */
export class ExploreStationSettings {
	enabled = $state(true);
	hidden = $state<string[]>([]);
	scenes = $state<VideoStationScene[]>([]);
	known = $state(false);
	busy = $state(false);
	error = $state('');

	async load() {
		if (this.busy) return;
		this.busy = true; this.error = '';
		try {
			this.apply(await api.getVideoStationSettings());
			this.known = true;
		} catch {
			this.known = false; this.error = 'Could not read the Explore stations setting.';
		} finally { this.busy = false; }
	}

	isOn(slug: string): boolean {
		return !this.hidden.includes(slug);
	}

	setEnabled(enabled: boolean) {
		return this.save(enabled, this.hidden);
	}

	toggleScene(slug: string) {
		const hidden = this.isOn(slug) ? [...this.hidden, slug] : this.hidden.filter((s) => s !== slug);
		return this.save(this.enabled, hidden);
	}

	private apply(settings: { enabled: boolean; hidden: string[]; scenes: VideoStationScene[] }) {
		this.enabled = settings.enabled;
		this.hidden = settings.hidden;
		this.scenes = settings.scenes;
	}

	private async save(enabled: boolean, hidden: string[]) {
		if (!this.known || this.busy) return;
		const previous = { enabled: this.enabled, hidden: this.hidden };
		this.enabled = enabled; this.hidden = hidden; this.busy = true; this.error = '';
		try { this.apply(await api.setVideoStationSettings({ enabled, hidden })); }
		catch { this.enabled = previous.enabled; this.hidden = previous.hidden; this.error = 'Could not save the Explore stations setting.'; }
		finally { this.busy = false; }
	}
}
