<script lang="ts">
	import { authFetch, getApiBase } from '$lib/api/client';
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

	let apiKey = $state('');
	let saving = $state(false);
	let errorMsg = $state('');
	let connected = $state(false);

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
			} else {
				errorMsg = data.message ?? 'Failed to save Last.fm API key.';
			}
		} catch (e) {
			errorMsg = e instanceof Error ? e.message : String(e);
		} finally {
			saving = false;
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
			<p class="error">{errorMsg}</p>
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
		font-size: 22px;
		font-weight: 600;
		letter-spacing: -0.01em;
	}
	.muted {
		margin: 0;
		max-width: 460px;
		color: var(--text-muted, #8b93a7);
		line-height: 1.5;
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
	.key-input:focus { outline: none; border-color: #4a6dd8; }
	.actions {
		display: flex;
		gap: 12px;
		justify-content: center;
		flex-wrap: wrap;
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
	.btn-primary { background: #4a6dd8; color: #fff; }
	.btn-primary:hover:not(:disabled) { background: #5a7ce8; }
	.btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }
	.btn-ghost {
		background: transparent;
		color: var(--text-muted, #8b93a7);
		border-color: rgba(255, 255, 255, 0.08);
	}
	.btn-ghost:hover { background: rgba(255, 255, 255, 0.04); color: #e7eaf2; }
	.error { color: #ff8a8a; margin: 0; }
	.success { color: #7fd99c; margin: 0; }
</style>
