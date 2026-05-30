<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type LastfmStatus, type ListenBrainzStatus } from '$lib/api/client';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import { openExternal } from '$lib/util/external';

	let lastfm = $state<LastfmStatus | null>(null);
	let listenbrainz = $state<ListenBrainzStatus | null>(null);
	let lastfmApiKey = $state('');
	let lastfmApiSecret = $state('');
	let listenbrainzToken = $state('');
	let lastfmMessage = $state('');
	let listenbrainzMessage = $state('');
	let backfillMessage = $state('');
	let busy = $state<string | null>(null);

	onMount(() => {
		void refresh();
	});

	function lastfmBadgeLabel(status: LastfmStatus | null): string {
		if (status?.scrobbling) return 'Connected';
		if (status?.api_key_configured && !status.api_secret_configured) return 'Needs secret';
		if (status?.api_key_configured && status.api_secret_configured) return 'Needs auth';
		return 'Not set up';
	}

	function lastfmBadgeTone(status: LastfmStatus | null): 'success' | 'warning' | 'muted' {
		if (status?.scrobbling) return 'success';
		if (status?.api_key_configured) return 'warning';
		return 'muted';
	}

	function lastfmStatusLine(status: LastfmStatus | null): string {
		if (status?.scrobbling && status.user) {
			return `Scrobbling as @${status.user}. ${status.pending_submissions ?? 0} pending, ${status.failed_submissions ?? 0} failed.`;
		}
		if (status?.api_key_configured && !status.api_secret_configured) {
			return 'API key saved. Add the Last.fm API secret, save credentials, then start account auth.';
		}
		if (status?.api_key_configured && status.api_secret_configured) {
			return 'Credentials saved. Start account auth, approve in Last.fm, then complete auth here.';
		}
		return 'Save your own Last.fm API key and API secret to enable auth, scrobbling, and recommendations.';
	}

	function connectedProviderCount(): number {
		return Number(Boolean(lastfm?.scrobbling)) + Number(Boolean(listenbrainz?.scrobbling));
	}

	function canSaveLastfmConfig(): boolean {
		if (busy !== null) return false;
		if (lastfmApiKey.trim()) return true;
		return Boolean(lastfm?.api_key_configured && lastfmApiSecret.trim());
	}

	function lastfmSaveLabel(): string {
		if (busy === 'lastfm-save') return 'Saving...';
		if (lastfmApiKey.trim()) return lastfm?.api_key_configured ? 'Save API key' : 'Save credentials';
		return 'Save secret';
	}

	function pendingSubmissionCount(): number {
		return Math.max(lastfm?.pending_submissions ?? 0, listenbrainz?.pending_submissions ?? 0);
	}

	function failedSubmissionCount(): number {
		return Math.max(lastfm?.failed_submissions ?? 0, listenbrainz?.failed_submissions ?? 0);
	}

	function uploadBadgeLabel(): string {
		if (connectedProviderCount() === 0) return 'Not connected';
		if (pendingSubmissionCount() > 0) return 'Uploading';
		if (failedSubmissionCount() > 0) return 'Needs attention';
		return 'Clear';
	}

	function uploadBadgeTone(): 'active' | 'success' | 'warning' | 'error' | 'muted' {
		if (connectedProviderCount() === 0) return 'muted';
		if (pendingSubmissionCount() > 0) return 'active';
		if (failedSubmissionCount() > 0) return 'error';
		return 'success';
	}

	function uploadStatusLine(): string {
		const pending = pendingSubmissionCount();
		const failed = failedSubmissionCount();
		if (connectedProviderCount() === 0) {
			return 'Connect Last.fm or ListenBrainz before uploading scrobbles.';
		}
		if (pending > 0) {
			const failedText = failed > 0 ? ` ${failed.toLocaleString()} failed after retries.` : '';
			return `Uploading ${pending.toLocaleString()} queued scrobbles in the background while NOORwave is running.${failedText}`;
		}
		if (failed > 0) {
			return `${failed.toLocaleString()} submissions failed after retries. Check the provider connection before backfilling again.`;
		}
		return 'Scrobble queue is clear. New eligible listens upload automatically.';
	}

	async function refresh() {
		const [lastfmStatus, listenbrainzStatus] = await Promise.allSettled([
			api.getLastfmStatus(),
			api.getListenBrainzStatus(),
		]);
		if (lastfmStatus.status === 'fulfilled') lastfm = lastfmStatus.value;
		if (listenbrainzStatus.status === 'fulfilled') listenbrainz = listenbrainzStatus.value;
	}

	async function refreshStatus() {
		busy = 'refresh';
		try {
			await refresh();
		} finally {
			busy = null;
		}
	}

	async function saveLastfm() {
		busy = 'lastfm-save';
		lastfmMessage = '';
		try {
			const result = await api.saveLastfmConfig(lastfmApiKey.trim(), lastfmApiSecret.trim());
			if (result.status === 'ok') {
				lastfmApiKey = '';
				lastfmApiSecret = '';
				lastfmMessage = 'Last.fm API settings saved.';
				await refresh();
			} else {
				lastfmMessage = result.message ?? 'Last.fm rejected those credentials.';
			}
		} catch (error) {
			lastfmMessage = error instanceof Error ? error.message : 'Last.fm setup failed.';
		} finally {
			busy = null;
		}
	}

	async function startLastfmAuth() {
		busy = 'lastfm-auth';
		lastfmMessage = '';
		try {
			const result = await api.lastfmAuthStart();
			if (result.status === 'awaiting' && result.auth_url) {
				await openExternal(result.auth_url);
				lastfmMessage = 'Authorize NOORwave in Last.fm, then return here and complete the connection.';
			} else {
				lastfmMessage = result.message ?? 'Could not start Last.fm auth.';
			}
		} catch (error) {
			lastfmMessage = error instanceof Error ? error.message : 'Could not start Last.fm auth.';
		} finally {
			busy = null;
		}
	}

	async function completeLastfmAuth() {
		busy = 'lastfm-complete';
		lastfmMessage = '';
		try {
			const result = await api.lastfmAuthComplete();
			if (result.status === 'connected') {
				lastfmMessage = `Last.fm connected as @${result.user ?? 'you'}.`;
				await refresh();
			} else {
				lastfmMessage = result.message ?? 'Last.fm auth is not complete yet.';
			}
		} catch (error) {
			lastfmMessage = error instanceof Error ? error.message : 'Could not complete Last.fm auth.';
		} finally {
			busy = null;
		}
	}

	async function disconnectLastfm() {
		busy = 'lastfm-disconnect';
		try {
			await api.lastfmAuthDisconnect();
			lastfmMessage = 'Last.fm account disconnected.';
			await refresh();
		} finally {
			busy = null;
		}
	}

	async function clearLastfm() {
		busy = 'lastfm-clear';
		try {
			await api.clearLastfmConfig();
			lastfmMessage = 'Last.fm credentials cleared.';
			await refresh();
		} finally {
			busy = null;
		}
	}

	async function saveListenBrainz() {
		busy = 'listenbrainz-save';
		listenbrainzMessage = '';
		try {
			const result = await api.saveListenBrainzConfig(listenbrainzToken.trim());
			if (result.status === 'ok') {
				listenbrainzToken = '';
				listenbrainzMessage = `ListenBrainz connected as @${result.user ?? 'you'}.`;
				await refresh();
			} else {
				listenbrainzMessage = result.message ?? 'ListenBrainz rejected that token.';
			}
		} catch (error) {
			listenbrainzMessage = error instanceof Error ? error.message : 'ListenBrainz setup failed.';
		} finally {
			busy = null;
		}
	}

	async function clearListenBrainz() {
		busy = 'listenbrainz-clear';
		try {
			await api.clearListenBrainzConfig();
			listenbrainzMessage = 'ListenBrainz token cleared.';
			await refresh();
		} finally {
			busy = null;
		}
	}

	async function backfill() {
		busy = 'backfill';
		backfillMessage = '';
		try {
			const result = await api.backfillScrobbles();
			if (result.queued > 0) {
				backfillMessage = `Queued ${result.queued.toLocaleString()} provider submissions from the last ${result.days} days. Uploading continues in the background.`;
			} else if (result.status === 'up_to_date' || connectedProviderCount() > 0 || (result.providers ?? 0) > 0) {
				backfillMessage = `No new backfill submissions were queued. The last ${result.days} days are already queued or submitted.`;
			} else if ((result.eligible ?? 0) > 0) {
				backfillMessage = 'No provider is connected for scrobbling yet. Finish Last.fm auth or connect ListenBrainz, then backfill.';
			} else {
				backfillMessage = `No eligible listens found in the last ${result.days} days.`;
			}
			await refresh();
		} catch (error) {
			backfillMessage = error instanceof Error ? error.message : 'Backfill failed.';
		} finally {
			busy = null;
		}
	}
</script>

<section class="glass-panel section-panel integrations-panel" id="integrations-listening">
	<SectionHeader
		eyebrow="Listening"
		title="Scrobbling and recommendations"
		subtitle="Send listens to your own Last.fm and ListenBrainz profiles."
	/>

	<div class="privacy-note">
		<strong>Opt-in only.</strong>
		<span>Provider profiles may be public. Disable other scrobblers for the same source to avoid duplicates.</span>
	</div>

	<div class="integration-grid">
		<section class="integration-card">
			<header>
				<div>
					<h3>Last.fm</h3>
					<p>Scrobbles, loves, and profile-derived recommendations.</p>
				</div>
				<StateBadge
					label={lastfmBadgeLabel(lastfm)}
					tone={lastfmBadgeTone(lastfm)}
				/>
			</header>

			<div class="field-grid">
				<label>
					<span>API key</span>
					<input type="password" bind:value={lastfmApiKey} autocomplete="off" />
				</label>
				<label>
					<span>API secret</span>
					<input type="password" bind:value={lastfmApiSecret} autocomplete="off" />
				</label>
			</div>

			<div class="integration-actions">
				<button
					class="btn btn-primary"
					onclick={saveLastfm}
					disabled={!canSaveLastfmConfig()}
				>
					{lastfmSaveLabel()}
				</button>
				<button class="btn btn-glass" onclick={() => void openExternal('https://www.last.fm/api/account/create')}>Create app</button>
			</div>

			{#if lastfm?.api_key_configured && lastfm?.api_secret_configured}
				<div class="integration-actions">
					<button class="btn btn-glass" onclick={startLastfmAuth} disabled={busy !== null}>Start account auth</button>
					<button class="btn btn-glass" onclick={completeLastfmAuth} disabled={busy !== null}>Complete auth</button>
					{#if lastfm.scrobbling}
						<button class="btn btn-glass" onclick={disconnectLastfm} disabled={busy !== null}>Disconnect account</button>
					{/if}
					<button class="btn btn-glass" onclick={clearLastfm} disabled={busy !== null}>Clear credentials</button>
				</div>
			{/if}

			<p class="status-line">
				{lastfmStatusLine(lastfm)}
			</p>
			{#if lastfmMessage}
				<p class="message-line">{lastfmMessage}</p>
			{/if}
		</section>

		<section class="integration-card">
			<header>
				<div>
					<h3>ListenBrainz</h3>
					<p>Open listening history and collaborative recommendations.</p>
				</div>
				<StateBadge
					label={listenbrainz?.scrobbling ? 'Connected' : 'Not set up'}
					tone={listenbrainz?.scrobbling ? 'success' : 'muted'}
				/>
			</header>

			<label class="token-field">
				<span>User token</span>
				<input type="password" bind:value={listenbrainzToken} autocomplete="off" />
			</label>

			<div class="integration-actions">
				<button class="btn btn-primary" onclick={saveListenBrainz} disabled={busy !== null || !listenbrainzToken.trim()}>
					{busy === 'listenbrainz-save' ? 'Validating...' : 'Save token'}
				</button>
				<button class="btn btn-glass" onclick={() => void openExternal('https://listenbrainz.org/profile/')}>Find token</button>
				{#if listenbrainz?.configured}
					<button class="btn btn-glass" onclick={clearListenBrainz} disabled={busy !== null}>Disconnect</button>
				{/if}
			</div>

			<p class="status-line">
				{#if listenbrainz?.user}
					Scrobbling as @{listenbrainz.user}. {listenbrainz.pending_submissions ?? 0} pending, {listenbrainz.failed_submissions ?? 0} failed.
				{:else}
					Paste your ListenBrainz user token to enable scrobbling and recommendations.
				{/if}
			</p>
			{#if listenbrainzMessage}
				<p class="message-line">{listenbrainzMessage}</p>
			{/if}
		</section>
	</div>

	<div class="upload-row">
		<div>
			<div class="row-title">
				<strong>Upload status</strong>
				<StateBadge label={uploadBadgeLabel()} tone={uploadBadgeTone()} compact />
			</div>
			<p>{uploadStatusLine()}</p>
		</div>
		<button class="btn btn-glass" onclick={refreshStatus} disabled={busy !== null}>
			{busy === 'refresh' ? 'Refreshing...' : 'Refresh status'}
		</button>
	</div>

	<div class="backfill-row">
		<div>
			<strong>Manual backfill</strong>
			<p>Queue eligible listens from the last 30 days. Nothing older is submitted in v1.</p>
		</div>
		<button class="btn btn-glass" onclick={backfill} disabled={busy !== null}>
			{busy === 'backfill' ? 'Queueing...' : 'Backfill 30 days'}
		</button>
	</div>
	{#if backfillMessage}
		<p class="message-line">{backfillMessage}</p>
	{/if}
</section>

<style>
	.integrations-panel {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		width: 100%;
		align-self: stretch;
		padding: var(--space-4);
	}

	.privacy-note,
	.upload-row,
	.backfill-row,
	.integration-card {
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		background: var(--panel-bg);
	}

	.privacy-note {
		display: flex;
		gap: var(--space-2);
		align-items: flex-start;
		padding: var(--space-4);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.privacy-note strong,
	.upload-row strong,
	.backfill-row strong,
	.integration-card h3 {
		color: var(--text-primary);
	}

	.integration-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(min(420px, 100%), 1fr));
		gap: var(--space-4);
	}

	.integration-card {
		padding: var(--space-4);
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		min-width: 0;
	}

	.integration-card header,
	.upload-row,
	.backfill-row {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-4);
	}

	.integration-card header > div {
		min-width: 0;
	}

	.integration-card h3,
	.integration-card p,
	.upload-row p,
	.backfill-row p,
	.status-line,
	.message-line {
		margin: 0;
	}

	.integration-card h3 {
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
	}

	.integration-card p,
	.upload-row p,
	.backfill-row p,
	.status-line,
	.message-line {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.field-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(min(220px, 100%), 1fr));
		gap: var(--space-3);
	}

	label {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	input {
		width: 100%;
		box-sizing: border-box;
		border: 1px solid var(--border-muted);
		border-radius: var(--radius-sm);
		background: var(--bg-surface);
		color: var(--text-primary);
		padding: var(--space-2) var(--space-3);
		font-family: inherit;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.integration-actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-3);
	}

	.upload-row,
	.backfill-row {
		padding: var(--space-4);
		align-items: center;
		flex-wrap: wrap;
	}

	.row-title {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin-bottom: var(--space-1);
	}

	.upload-row .btn,
	.backfill-row .btn {
		margin-left: auto;
	}

	.message-line {
		color: var(--state-active);
	}

	@media (max-width: 720px) {
		.integrations-panel {
			padding: var(--space-4);
		}

		.integration-card header,
		.upload-row,
		.backfill-row,
		.privacy-note {
			flex-direction: column;
		}

		.upload-row .btn,
		.backfill-row .btn {
			margin-left: 0;
		}
	}
</style>
