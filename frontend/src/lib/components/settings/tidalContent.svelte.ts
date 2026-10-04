import { api } from '$lib/api/client';
import { invalidateTidalContentCaches } from '$lib/cache/ws_events';

export class TidalContentSettings {
    enabled = $state(false);
    known = $state(false);
    busy = $state(false);
    error = $state('');

    async load() {
        if (this.busy) return;
        this.busy = true; this.error = '';
        try { this.enabled = (await api.getTidalContentSettings()).hide_ai_generated; this.known = true; }
        catch { this.known = false; this.error = 'Could not read the AI music filter setting.'; }
        finally { this.busy = false; }
    }

    async save(next: boolean) {
        if (!this.known || this.busy) return;
        const previous = this.enabled;
        this.enabled = next; this.busy = true; this.error = '';
        try {
            this.enabled = (await api.setTidalContentSettings(next)).hide_ai_generated;
            invalidateTidalContentCaches();
        } catch { this.enabled = previous; this.error = 'Could not save the AI music filter setting.'; }
        finally { this.busy = false; }
    }
}
