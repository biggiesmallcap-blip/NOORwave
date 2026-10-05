import { api, type LastfmStatus, type ListenBrainzStatus } from '$lib/api/client';
import { cacheKeys, seedCachedValue } from '$lib/cache/api_queries';
import { openExternal } from '$lib/util/external';

export function lastfmConnectionLabel(status: LastfmStatus | null): string {
	if (!status) return 'Checking…';
	if (status.scrobbling) return 'Connected';
	if (status.api_key_configured && status.api_secret_configured) return 'Credentials saved';
	if (status.api_key_configured || status.enrichment) return 'Tags ready';
	return 'Not connected';
}

/** Shared behaviour; each mounted setup owns its transient fields and operations. */
export class ListeningServices {
	lastfm = $state<LastfmStatus | null>(null);
	listenbrainz = $state<ListenBrainzStatus | null>(null);
	lastfmError = $state('');
	listenbrainzError = $state('');
	lastfmMessage = $state('');
	listenbrainzMessage = $state('');
	busy = $state<string | null>(null);
	loading = $state(true);
	authUrl = $state('');
	approvalPending = $state(false);
	private refreshGeneration = 0;

	async refresh() {
		const generation = ++this.refreshGeneration;
		this.loading = true;
		const results = await Promise.allSettled([api.getLastfmStatus(), api.getListenBrainzStatus()]);
		if (generation !== this.refreshGeneration) return;
		if (results[0].status === 'fulfilled') {
			this.lastfm = results[0].value; this.lastfmError = '';
			seedCachedValue(cacheKeys.settings.lastfmStatus(), results[0].value);
		} else this.lastfmError = 'Last.fm status is unavailable. Retry to confirm the connection.';
		if (results[1].status === 'fulfilled') {
			this.listenbrainz = results[1].value; this.listenbrainzError = '';
			seedCachedValue(cacheKeys.settings.listenBrainzStatus(), results[1].value);
		} else this.listenbrainzError = 'ListenBrainz status is unavailable. Retry to confirm the connection.';
		this.loading = false;
	}

	async run(id: string, provider: 'lastfm' | 'listenbrainz', operation: () => Promise<void>): Promise<boolean> {
		if (this.busy) return false;
		this.busy = id;
		this[provider === 'lastfm' ? 'lastfmError' : 'listenbrainzError'] = '';
		this[provider === 'lastfm' ? 'lastfmMessage' : 'listenbrainzMessage'] = '';
		try { await operation(); return true; }
		catch (error) {
			this[provider === 'lastfm' ? 'lastfmError' : 'listenbrainzError'] = error instanceof Error ? error.message : 'Connection could not be updated.';
			return false;
		} finally { this.busy = null; }
	}
	async saveLastfm(key: string, secret: string) {
		return this.run('lastfm-save', 'lastfm', async () => {
			const result = await api.saveLastfmConfig(key.trim(), secret.trim());
			if (result.status !== 'ok') throw new Error(result.message ?? 'Last.fm rejected these credentials.');
			await this.refresh();
			this.lastfmMessage = 'Credentials saved. Connect your account to enable profile recommendations and listening uploads.';
		});
	}
	async startLastfm() {
		return this.run('lastfm-start', 'lastfm', async () => {
			const result = await api.lastfmAuthStart();
			if (result.status !== 'awaiting' || !result.auth_url) throw new Error(result.message ?? 'Could not start account approval.');
			this.authUrl = result.auth_url; this.approvalPending = true;
			const opened = await openExternal(result.auth_url);
			if (!opened.ok) throw new Error(opened.error + ' Use the approval link below.');
			this.lastfmMessage = 'Approve NOORwave in Last.fm, then return and finish the connection.';
		});
	}
	async finishLastfm() {
		return this.run('lastfm-finish', 'lastfm', async () => {
			const result = await api.lastfmAuthComplete();
			if (result.status !== 'connected') throw new Error(result.message ?? 'Account approval is not complete yet.');
			this.approvalPending = false; this.authUrl = '';
			await this.refresh();
			this.lastfmMessage = 'Account approval confirmed.';
		});
	}
	async disconnectLastfm(removeCredentials = false) {
		return this.run('lastfm-disconnect', 'lastfm', async () => {
			const result = removeCredentials ? await api.clearLastfmConfig() : await api.lastfmAuthDisconnect();
			if (!['ok', 'disconnected', 'cleared'].includes(result.status)) throw new Error('Last.fm could not be disconnected.');
			this.approvalPending = false; this.authUrl = '';
			await this.refresh();
			this.lastfmMessage = removeCredentials ? 'Last.fm credentials removed.' : 'Account disconnected. Tag credentials are retained.';
		});
	}
	async saveListenBrainz(token: string) {
		return this.run('listenbrainz-save', 'listenbrainz', async () => {
			const result = await api.saveListenBrainzConfig(token.trim());
			if (result.status !== 'ok') throw new Error(result.message ?? 'ListenBrainz rejected this token.');
			await this.refresh();
			this.listenbrainzMessage = 'Token validated.';
		});
	}
	async disconnectListenBrainz() {
		return this.run('listenbrainz-disconnect', 'listenbrainz', async () => {
			const result = await api.clearListenBrainzConfig();
			if (!['ok', 'disconnected', 'cleared'].includes(result.status)) throw new Error('ListenBrainz could not be disconnected.');
			await this.refresh();
			this.listenbrainzMessage = 'ListenBrainz disconnected.';
		});
	}
}
