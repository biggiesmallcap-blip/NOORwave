<script lang="ts">
	import { api } from '$lib/api/client';
	import { openExternal } from '$lib/util/external';

	let {
		oncontinue,
		onskip
	}: {
		oncontinue?: () => void;
		onskip?: () => void;
	} = $props();

	let lastfmApiKey = $state('');
	let lastfmApiSecret = $state('');
	let lastfmBusy = $state(false);
	let lastfmMessage = $state('');
	let lastfmUser = $state<string | null>(null);

	let listenBrainzToken = $state('');
	let listenBrainzBusy = $state(false);
	let listenBrainzMessage = $state('');
	let listenBrainzUser = $state<string | null>(null);

	async function saveLastfm() {
		if (!lastfmApiKey.trim() || !lastfmApiSecret.trim()) {
			lastfmMessage = 'Paste your Last.fm API key and shared secret first.';
			return;
		}
		lastfmBusy = true;
		lastfmMessage = '';
		try {
			await api.saveLastfmConfig(lastfmApiKey.trim(), lastfmApiSecret.trim());
			const auth = await api.lastfmAuthStart();
			if (auth.auth_url) {
				void openExternal(auth.auth_url);
				lastfmMessage = 'Approve NOORwave in Last.fm, then come back and finish here.';
			} else {
				lastfmMessage = auth.message ?? 'Credentials saved. Start account auth from Settings.';
			}
		} catch (err) {
			lastfmMessage = err instanceof Error ? err.message : 'Could not save Last.fm credentials.';
		} finally {
			lastfmBusy = false;
		}
	}

	async function completeLastfm() {
		lastfmBusy = true;
		lastfmMessage = '';
		try {
			const response = await api.lastfmAuthComplete();
			if (response.status === 'connected') {
				lastfmUser = response.user ?? null;
				lastfmMessage = response.user ? `Connected as ${response.user}.` : 'Last.fm connected.';
			} else {
				lastfmMessage = response.message ?? 'Last.fm has not approved this app yet.';
			}
		} catch (err) {
			lastfmMessage = err instanceof Error ? err.message : 'Could not complete Last.fm auth.';
		} finally {
			lastfmBusy = false;
		}
	}

	async function saveListenBrainz() {
		if (!listenBrainzToken.trim()) {
			listenBrainzMessage = 'Paste your ListenBrainz token first.';
			return;
		}
		listenBrainzBusy = true;
		listenBrainzMessage = '';
		try {
			const response = await api.saveListenBrainzConfig(listenBrainzToken.trim());
			listenBrainzUser = response.user ?? null;
			listenBrainzToken = '';
			listenBrainzMessage = response.user ? `Connected as ${response.user}.` : 'ListenBrainz connected.';
		} catch (err) {
			listenBrainzMessage = err instanceof Error ? err.message : 'Could not validate ListenBrainz token.';
		} finally {
			listenBrainzBusy = false;
		}
	}
</script>

<div class="listening-services">
	<div class="intro">
		<h2>Connect listening history</h2>
		<p>
			Optional scrobbling sends eligible plays to the profiles you choose. NOORwave does not use a shared
			Last.fm secret, and provider failures never stop local playback.
		</p>
	</div>

	<div class="provider-grid">
		<section class="provider-card">
			<div class="provider-head">
				<h3>Last.fm</h3>
				<span>{lastfmUser ? 'Connected' : 'Optional'}</span>
			</div>
			<p>Use your own API key and shared secret, then authorize your account for scrobbling.</p>
			<div class="field-stack">
				<input bind:value={lastfmApiKey} placeholder="API key" autocomplete="off" />
				<input bind:value={lastfmApiSecret} placeholder="Shared secret" type="password" autocomplete="off" />
			</div>
			<div class="actions">
				<button type="button" class="btn btn-primary" onclick={saveLastfm} disabled={lastfmBusy}>
					{lastfmBusy ? 'Working...' : 'Save and authorize'}
				</button>
				<button type="button" class="btn btn-glass" onclick={completeLastfm} disabled={lastfmBusy}>
					I approved it
				</button>
				<button type="button" class="link-btn" onclick={() => void openExternal('https://www.last.fm/api/account/create')}>
					Create API app
				</button>
			</div>
			{#if lastfmMessage}
				<p class="status-line">{lastfmMessage}</p>
			{/if}
		</section>

		<section class="provider-card">
			<div class="provider-head">
				<h3>ListenBrainz</h3>
				<span>{listenBrainzUser ? 'Connected' : 'Optional'}</span>
			</div>
			<p>Paste a user token to enable scrobbling and profile recommendation shelves.</p>
			<div class="field-stack">
				<input bind:value={listenBrainzToken} placeholder="User token" type="password" autocomplete="off" />
			</div>
			<div class="actions">
				<button type="button" class="btn btn-primary" onclick={saveListenBrainz} disabled={listenBrainzBusy}>
					{listenBrainzBusy ? 'Validating...' : 'Connect token'}
				</button>
				<button type="button" class="link-btn" onclick={() => void openExternal('https://listenbrainz.org/profile/')}>
					Find token
				</button>
			</div>
			{#if listenBrainzMessage}
				<p class="status-line">{listenBrainzMessage}</p>
			{/if}
		</section>
	</div>

	<div class="footer-actions">
		<button type="button" class="btn btn-primary" onclick={() => oncontinue?.()}>Continue</button>
		<button type="button" class="btn btn-glass" onclick={() => onskip?.()}>Skip for now</button>
	</div>
</div>

<style>
	.listening-services {
		display: flex;
		flex-direction: column;
		gap: 20px;
	}

	.intro {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	h2,
	h3,
	p {
		margin: 0;
	}

	.intro p,
	.provider-card p,
	.status-line {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.provider-grid {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 14px;
	}

	.provider-card {
		display: flex;
		flex-direction: column;
		gap: 12px;
		border: 1px solid var(--border-subtle);
		border-radius: 8px;
		background: rgba(255, 255, 255, 0.04);
		padding: 16px;
		min-width: 0;
	}

	.provider-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
	}

	.provider-head span {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.field-stack {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	input {
		width: 100%;
		min-width: 0;
		box-sizing: border-box;
		border: 1px solid var(--border-subtle);
		border-radius: 8px;
		background: rgba(0, 0, 0, 0.2);
		color: var(--text-primary);
		padding: 10px 12px;
		font-family: inherit;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.actions,
	.footer-actions {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 10px;
	}

	.link-btn {
		border: 0;
		background: transparent;
		color: var(--accent);
		padding: 0;
		font-family: inherit;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
		cursor: pointer;
	}

	@media (max-width: 780px) {
		.provider-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
