<script lang="ts">
	import { onMount } from 'svelte';
	import { authFetch, getApiBase, api } from '$lib/api/client';
	import { openExternal } from '$lib/util/external';

	let {
		variant = 'onboarding',
		showSkip = true,
		onconnected,
		onskip,
	}: {
		variant?: 'onboarding' | 'settings';
		showSkip?: boolean;
		onconnected?: () => void;
		onskip?: () => void;
	} = $props();

	// ─── Tag-enrichment / API-key state ──────────────────────────────────
	let apiKey = $state('');
	let saving = $state(false);
	let errorMsg = $state('');
	let connected = $state(false);

	// ─── Scrobble auth state ─────────────────────────────────────────────
	// Mirrors the TidalConnect state machine in spirit, but for web-auth
	// (NOT device-flow polling). We open the Last.fm authorize page, then
	// the user clicks "I've authorized" to redeem the token server-side.
	type ScrobbleState =
		| 'idle'
		| 'starting'
		| 'awaiting_user'
		| 'completing'
		| 'connected'
		| 'error'
		| 'unavailable'; // Server has no LASTFM_API_SECRET set.
	let scrobbleState = $state<ScrobbleState>('idle');
	let scrobbleUser = $state<string | null>(null);
	let scrobbleAvailable = $state(false);
	let scrobbleError = $state('');
	let pendingAuthUrl = $state<string | null>(null);

	onMount(loadStatus);

	async function loadStatus() {
		try {
			const status = await api.getLastfmStatus();
			connected = status.enrichment;
			scrobbleAvailable = status.scrobble_available;
			scrobbleUser = status.user;
			if (!status.scrobble_available) {
				scrobbleState = 'unavailable';
			} else if (status.scrobbling) {
				scrobbleState = 'connected';
			} else {
				scrobbleState = 'idle';
			}
		} catch {
			// Status endpoint failure shouldn't block the form — fall back to
			// the legacy flow (start with `connected = false`).
		}
	}

	async function save() {
		if (!apiKey.trim()) {
			errorMsg = 'Paste your Last.fm API key first.';
			return;
		}
		saving = true;
		errorMsg = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/lastfm/config`, {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ api_key: apiKey.trim() }),
			});
			const data = await resp.json();
			if (data.status === 'ok') {
				connected = true;
				apiKey = '';
				onconnected?.();
				// Refresh scrobble availability after the API key lands —
				// it gates the scrobble auth flow.
				void loadStatus();
			} else {
				errorMsg = data.message ?? 'Failed to save Last.fm API key.';
			}
		} catch (e) {
			errorMsg = e instanceof Error ? e.message : String(e);
		} finally {
			saving = false;
		}
	}

	async function startScrobbleAuth() {
		scrobbleState = 'starting';
		scrobbleError = '';
		try {
			const data = await api.lastfmAuthStart();
			if (data.status === 'awaiting' && data.auth_url) {
				pendingAuthUrl = data.auth_url;
				scrobbleState = 'awaiting_user';
				openExternal(data.auth_url);
			} else {
				scrobbleState = 'error';
				scrobbleError = data.message ?? 'Could not start Last.fm auth.';
			}
		} catch (e: unknown) {
			// 501 → server has no LASTFM_API_SECRET configured.
			const status = (e as { status?: number })?.status;
			if (status === 501) {
				scrobbleState = 'unavailable';
			} else {
				scrobbleState = 'error';
				scrobbleError = e instanceof Error ? e.message : 'Failed to start auth.';
			}
		}
	}

	async function completeScrobbleAuth() {
		scrobbleState = 'completing';
		scrobbleError = '';
		try {
			const data = await api.lastfmAuthComplete();
			if (data.status === 'connected') {
				scrobbleUser = data.user ?? null;
				scrobbleState = 'connected';
				pendingAuthUrl = null;
			} else if (data.status === 'not_yet_authorized') {
				// Stay in awaiting state so the user can retry after authorizing.
				scrobbleState = 'awaiting_user';
				scrobbleError = 'Not yet authorized — click "Yes, allow access" on Last.fm first.';
			} else {
				scrobbleState = 'error';
				scrobbleError = data.message ?? 'Failed to complete Last.fm auth.';
			}
		} catch (e) {
			scrobbleState = 'error';
			scrobbleError = e instanceof Error ? e.message : 'Failed to complete auth.';
		}
	}

	async function cancelScrobbleAuth() {
		// Disconnect clears any stashed pending_token + session_key.
		try {
			await api.lastfmAuthDisconnect();
		} catch {
			// Best-effort — proceed regardless.
		}
		pendingAuthUrl = null;
		scrobbleState = scrobbleAvailable ? 'idle' : 'unavailable';
	}

	async function disconnectScrobbling() {
		try {
			await api.lastfmAuthDisconnect();
			scrobbleUser = null;
			scrobbleState = scrobbleAvailable ? 'idle' : 'unavailable';
		} catch (e) {
			scrobbleError = e instanceof Error ? e.message : 'Failed to disconnect.';
		}
	}
</script>

<div class="lastfm-connect" class:variant-onboarding={variant === 'onboarding'} class:variant-settings={variant === 'settings'}>
	{#if !connected}
		{#if variant === 'onboarding'}
			<h2>Add Last.fm tags</h2>
			<p class="muted">
				Optional: paste a free Last.fm API key to enrich your library with crowd-sourced tags
				and similar-artist suggestions.
				<button type="button" class="muted-link" onclick={() => openExternal('https://www.last.fm/api/account/create')}>Get one here</button>.
			</p>
		{/if}

		<input
			class="key-input"
			type="text"
			placeholder="Last.fm API key"
			bind:value={apiKey}
			disabled={saving}
			autocomplete="off"
			spellcheck="false"
		/>

		{#if errorMsg}
			<p class="error" role="alert">{errorMsg}</p>
		{/if}

		<div class="actions">
			<button class="btn btn-primary" onclick={save} disabled={saving || !apiKey.trim()}>
				{saving ? 'Saving…' : 'Save key'}
			</button>
			{#if showSkip}
				<button class="btn btn-ghost" onclick={() => onskip?.()}>Skip for now</button>
			{/if}
		</div>
	{:else}
		<p class="success">Last.fm key saved.</p>

		{#if variant === 'onboarding'}
			<div class="actions">
				<button class="btn btn-primary" onclick={() => onconnected?.()}>Continue</button>
			</div>
		{/if}

		<!-- Enable scrobbling sub-section. Only meaningful in the settings
		     variant — onboarding doesn't need to surface scrobble auth at
		     first-run. -->
		{#if variant === 'settings'}
			<div class="scrobble-section">
				<h3 class="scrobble-title">Enable scrobbling</h3>
				<p class="muted scrobble-copy">
					Scrobble TIDAL plays to your Last.fm profile. Eligible plays
					(<span class="nowrap">≥ 50% of duration</span> or
					<span class="nowrap">≥ 4 minutes</span>, with track ≥ 30s) are
					scrobbled automatically.
				</p>

				{#if scrobbleState === 'connected'}
					<p class="success">Scrobbling as @{scrobbleUser ?? 'you'}.</p>
					<div class="actions">
						<button class="btn btn-ghost" onclick={disconnectScrobbling}>Disconnect</button>
					</div>
				{:else if scrobbleState === 'unavailable'}
					<p class="muted">
						Server is not configured for Last.fm scrobbling. Ask the admin to set
						<code>LASTFM_API_SECRET</code>.
					</p>
				{:else if scrobbleState === 'awaiting_user'}
					<p class="muted">
						A Last.fm authorization tab opened. Click <strong>"Yes, allow access"</strong>
						there, then come back and click below.
					</p>
					{#if scrobbleError}
						<p class="error" role="alert">{scrobbleError}</p>
					{/if}
					<div class="actions">
						<button class="btn btn-primary" onclick={completeScrobbleAuth}>I've authorized</button>
						{#if pendingAuthUrl}
							<button class="btn btn-ghost" onclick={() => pendingAuthUrl && openExternal(pendingAuthUrl)}>
								Reopen auth page
							</button>
						{/if}
						<button class="btn btn-ghost" onclick={cancelScrobbleAuth}>Cancel</button>
					</div>
				{:else if scrobbleState === 'completing'}
					<p class="muted">Completing connection…</p>
				{:else if scrobbleState === 'starting'}
					<p class="muted">Starting Last.fm auth…</p>
				{:else}
					{#if scrobbleError}
						<p class="error" role="alert">{scrobbleError}</p>
					{/if}
					<div class="actions">
						<button class="btn btn-primary" onclick={startScrobbleAuth}>
							Connect Last.fm account
						</button>
					</div>
				{/if}
			</div>
		{/if}
	{/if}
</div>

<style>
	.lastfm-connect {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}
	.variant-onboarding {
		text-align: center;
		align-items: center;
	}
	.variant-onboarding h2 {
		margin: 0 0 4px;
		font-family: var(--font-display);
		font-size: clamp(1.75rem, 1.4rem + 1.4vw, 2.25rem);
		font-weight: 500;
		letter-spacing: -0.02em;
		line-height: 1.05;
	}
	.muted {
		margin: 0;
		max-width: 460px;
		color: var(--text-secondary);
		line-height: 1.55;
		font-size: var(--font-size-md);
	}
	.muted-link {
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		color: #8aa9ff;
		cursor: pointer;
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.key-input {
		font: inherit;
		padding: 10px 14px;
		border-radius: 8px;
		border: 1px solid rgba(255, 255, 255, 0.1);
		background: rgba(255, 255, 255, 0.03);
		color: inherit;
		min-width: 320px;
		max-width: 100%;
	}
	.key-input:focus { outline: none; border-color: rgba(255, 255, 255, 0.6); }
	.actions {
		display: flex;
		gap: 12px;
		justify-content: flex-start;
		flex-wrap: wrap;
	}
	.variant-onboarding .actions {
		justify-content: center;
	}
	.btn {
		font: inherit;
		padding: 10px 20px;
		border-radius: 8px;
		border: 1px solid transparent;
		cursor: pointer;
		font-weight: 500;
		transition: background 120ms, border-color 120ms;
	}
	.btn-primary { background: rgba(255, 255, 255, 0.92); color: #0a0d14; }
	.btn-primary:hover:not(:disabled) { background: #fff; }
	.btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }
	.btn-ghost {
		background: transparent;
		color: var(--text-muted, #8b93a7);
		border-color: rgba(255, 255, 255, 0.08);
	}
	.btn-ghost:hover { background: rgba(255, 255, 255, 0.04); color: #e7eaf2; }
	.error { color: #ff8a8a; margin: 0; }
	.success { color: #7fd99c; margin: 0; }
	.scrobble-section {
		margin-top: 8px;
		padding-top: 14px;
		border-top: 1px solid var(--border-subtle);
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.scrobble-title {
		margin: 0;
		font-size: 15px;
		font-weight: 600;
		letter-spacing: -0.005em;
	}
	.scrobble-copy {
		max-width: 540px;
	}
	.nowrap {
		white-space: nowrap;
	}
	code {
		font-family: var(--font-mono);
		background: rgba(255, 255, 255, 0.06);
		padding: 1px 5px;
		border-radius: 4px;
		font-size: 0.9em;
	}
</style>
