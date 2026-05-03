<script lang="ts">
	import { onDestroy } from 'svelte';
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
		onconnected?: (info: { user_id?: string }) => void;
		onskip?: () => void;
	} = $props();

	type Status = 'idle' | 'connecting' | 'awaiting' | 'connected' | 'error';

	let status = $state<Status>('idle');
	let userCode = $state('');
	let verifyUrl = $state('');
	let errorMsg = $state('');
	let pollTimer: ReturnType<typeof setInterval> | null = null;

	function clearPolling() {
		if (pollTimer !== null) {
			clearInterval(pollTimer);
			pollTimer = null;
		}
	}

	async function start() {
		status = 'connecting';
		errorMsg = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/tidal/login`, { method: 'POST' });
			if (!resp.ok) throw new Error(`Server returned ${resp.status}`);
			const data = await resp.json();
			userCode = data.user_code ?? '';
			verifyUrl = data.verify_url ?? '';
			status = 'awaiting';

			if (verifyUrl) openExternal(verifyUrl);

			pollTimer = setInterval(async () => {
				try {
					const pollResp = await authFetch(`${getApiBase()}/api/tidal/login/poll`, { method: 'POST' });
					const pollData = await pollResp.json();
					if (pollData.status === 'authenticated') {
						clearPolling();
						status = 'connected';
						onconnected?.({ user_id: pollData.user_id });
					}
				} catch {
					// Transient — keep polling.
				}
			}, 3000);
		} catch (e) {
			status = 'error';
			errorMsg = e instanceof Error ? e.message : String(e);
		}
	}

	function copyCode() {
		if (!userCode) return;
		navigator.clipboard?.writeText(userCode).catch(() => {});
	}

	function handleSkip() {
		clearPolling();
		onskip?.();
	}

	onDestroy(() => {
		clearPolling();
	});
</script>

<div class="tidal-connect" class:variant-onboarding={variant === 'onboarding'} class:variant-settings={variant === 'settings'}>
	{#if status === 'idle' || status === 'error'}
		<div class="prompt">
			{#if variant === 'onboarding'}
				<h2>Connect TIDAL</h2>
				<p>NOORwave plays from your TIDAL library. Sign in once and we'll keep your tracks in sync.</p>
			{/if}
			{#if errorMsg}
				<p class="error">{errorMsg}</p>
			{/if}
			<div class="actions">
				<button class="btn btn-primary" onclick={start}>
					{status === 'error' ? 'Try again' : 'Connect TIDAL'}
				</button>
				{#if showSkip}
					<button class="btn btn-ghost" onclick={handleSkip}>Skip for now</button>
				{/if}
			</div>
		</div>
	{:else if status === 'connecting'}
		<p class="muted">Asking TIDAL for a sign-in code…</p>
	{:else if status === 'awaiting'}
		<div class="device-code">
			<p class="muted">A browser tab opened to TIDAL. Enter this code there:</p>
			<div class="code" onclick={copyCode} role="button" tabindex="0" onkeydown={(e) => e.key === 'Enter' && copyCode()}>
				{userCode || '—'}
			</div>
			<p class="hint">
				Didn't open? <button type="button" class="hint-link" onclick={() => openExternal(verifyUrl)}>Open the page manually</button>.
			</p>
			{#if showSkip}
				<button class="btn btn-ghost" onclick={handleSkip}>Skip for now</button>
			{/if}
		</div>
	{:else if status === 'connected'}
		<p class="success">TIDAL connected.</p>
	{/if}
</div>

<style>
	.tidal-connect {
		display: flex;
		flex-direction: column;
		gap: 16px;
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
	.variant-onboarding p {
		margin: 0;
		max-width: 420px;
		color: var(--text-muted, #8b93a7);
		line-height: 1.5;
	}
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
	.btn-primary {
		background: #4a6dd8;
		color: #fff;
	}
	.btn-primary:hover:not(:disabled) { background: #5a7ce8; }
	.btn-primary:disabled { opacity: 0.6; cursor: not-allowed; }
	.btn-ghost {
		background: transparent;
		color: var(--text-muted, #8b93a7);
		border-color: rgba(255, 255, 255, 0.08);
	}
	.btn-ghost:hover { background: rgba(255, 255, 255, 0.04); color: #e7eaf2; }
	.code {
		font-family: 'SF Mono', Menlo, Consolas, monospace;
		font-size: 38px;
		font-weight: 600;
		letter-spacing: 0.18em;
		padding: 18px 28px;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.08);
		border-radius: 12px;
		cursor: pointer;
		user-select: all;
	}
	.device-code {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 12px;
	}
	.hint, .muted { color: var(--text-muted, #8b93a7); margin: 0; font-size: 13px; }
	.hint-link {
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		color: #8aa9ff;
		cursor: pointer;
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.error { color: #ff8a8a; margin: 0; }
	.success { color: #7fd99c; margin: 0; }
</style>
