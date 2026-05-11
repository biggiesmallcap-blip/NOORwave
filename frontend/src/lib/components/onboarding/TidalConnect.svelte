<script lang="ts">
	import { authFetch, getApiBase } from '$lib/api/client';
	import { openExternal } from '$lib/util/external';
	import { isValidTidalRedirectUrl, readTidalRedirectFromClipboard } from '$lib/tidal/login';

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
	let verifyUrl = $state('');
	let redirectUrl = $state('');
	let errorMsg = $state('');
	let redirectError = $state('');

	async function start() {
		status = 'connecting';
		errorMsg = '';
		redirectError = '';
		redirectUrl = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/tidal/login`, { method: 'POST' });
			if (!resp.ok) throw new Error(`Server returned ${resp.status}`);
			const data = await resp.json();
			verifyUrl = data.verify_url ?? '';
			status = 'awaiting';

			if (verifyUrl) openExternal(verifyUrl);
		} catch (e) {
			status = 'error';
			errorMsg = e instanceof Error ? e.message : String(e);
		}
	}

	async function completeLogin() {
		errorMsg = '';
		redirectError = '';
		if (!isValidTidalRedirectUrl(redirectUrl)) {
			redirectError = 'Paste the final TIDAL redirect URL to finish login.';
			return;
		}
		try {
			const resp = await authFetch(`${getApiBase()}/api/tidal/login/complete`, {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ redirect_url: redirectUrl.trim() }),
			});
			const data = await resp.json().catch(() => ({}));
			if (!resp.ok) throw new Error(data.error ?? `Server returned ${resp.status}`);
			status = 'connected';
			verifyUrl = '';
			redirectUrl = '';
			onconnected?.({ user_id: data.user_id });
		} catch (e) {
			status = 'error';
			errorMsg = e instanceof Error ? e.message : String(e);
		}
	}

	async function pasteRedirectUrl() {
		redirectError = '';
		const result = await readTidalRedirectFromClipboard();
		if (result.ok && result.redirectUrl) {
			redirectUrl = result.redirectUrl;
			return;
		}
		redirectError = result.error ?? 'Clipboard access failed. Paste the URL manually.';
	}

	function handleSkip() {
		onskip?.();
	}
</script>

<div class="tidal-connect" class:variant-onboarding={variant === 'onboarding'} class:variant-settings={variant === 'settings'}>
	{#if status === 'idle' || status === 'error'}
		<div class="prompt">
			{#if variant === 'onboarding'}
				<h2>Connect TIDAL</h2>
				<p>NOORwave plays from your TIDAL library. Sign in once and we'll keep your tracks in sync.</p>
			{/if}
			{#if errorMsg}
				<p class="error" role="alert">{errorMsg}</p>
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
		<p class="muted">Opening TIDAL sign-in...</p>
	{:else if status === 'awaiting'}
		<div class="redirect-login">
			<p class="muted">A TIDAL sign-in page opened.</p>
			<p class="muted">After sign-in, copy the address from the final TIDAL page. Paste it here to finish.</p>
			<input
				class="redirect-input"
				type="url"
				bind:value={redirectUrl}
				placeholder="https://tidal.com/android/login/auth?code=..."
			/>
			{#if redirectError}
				<p class="error" role="alert">{redirectError}</p>
			{/if}
			<div class="actions">
				<button class="btn btn-ghost" onclick={pasteRedirectUrl}>Paste from clipboard</button>
				<button class="btn btn-primary" onclick={completeLogin} disabled={!redirectUrl.trim()}>Finish login</button>
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
		font-family: var(--font-display);
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-medium);
		letter-spacing: 0;
		line-height: var(--line-height-tight);
	}
	.variant-onboarding p {
		margin: 0;
		max-width: 420px;
		color: var(--text-secondary);
		line-height: var(--line-height-loose);
		font-size: var(--font-size-md);
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
		font-weight: var(--font-weight-medium);
		transition: background 120ms, border-color 120ms;
	}
	.btn-primary {
		background: rgba(255, 255, 255, 0.92);
		color: #0a0d14;
	}
	.btn-primary:hover:not(:disabled) { background: #fff; }
	.btn-primary:disabled { opacity: 0.6; cursor: not-allowed; }
	.btn-ghost {
		background: transparent;
		color: var(--text-muted, #8b93a7);
		border-color: rgba(255, 255, 255, 0.08);
	}
	.btn-ghost:hover { background: rgba(255, 255, 255, 0.04); color: #e7eaf2; }
	.redirect-login {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 12px;
		width: min(100%, 520px);
	}
	.redirect-input {
		width: 100%;
		padding: 10px 12px;
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.04);
		color: var(--text-primary);
		font: inherit;
	}
	.hint, .muted { color: var(--text-tertiary); margin: 0; font-size: var(--font-size-xs); }
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
