<script lang="ts">
	import {
		pendingTidalLogin, startTidalLogin, completeTidalLogin,
		openTidalVerifyUrl, pasteTidalRedirectUrl, cancelTidalLogin
	} from '$lib/stores/tidalLogin';

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

	let connected = $state(false);
	let errorMsg = $state('');

	async function start() {
		errorMsg = '';
		try {
			await startTidalLogin();
		} catch (e) {
			errorMsg = e instanceof Error ? e.message : String(e);
		}
	}

	async function completeLogin() {
		errorMsg = '';
		try {
			const info = await completeTidalLogin();
			if (!info) return;
			connected = true;
			onconnected?.(info);
		} catch {
			// The shared pending form displays the error and remains available for retry.
		}
	}

	function handleSkip() {
		cancelTidalLogin();
		onskip?.();
	}
</script>

<div class="tidal-connect" class:variant-onboarding={variant === 'onboarding'} class:variant-settings={variant === 'settings'}>
	{#if !$pendingTidalLogin && !connected}
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
					{errorMsg ? 'Try again' : 'Connect TIDAL'}
				</button>
				{#if showSkip}
					<button class="btn btn-ghost" onclick={handleSkip}>Skip for now</button>
				{/if}
			</div>
		</div>
	{:else if $pendingTidalLogin?.phase === 'starting'}
		<p class="muted">Opening TIDAL sign-in...</p>
	{:else if $pendingTidalLogin}
		<div class="redirect-login">
			<p class="muted">Finish your TIDAL sign-in.</p>
			<p class="muted">After sign-in, copy the full address from the final TIDAL page, even if it says page not found. Paste it here to finish.</p>
			{#if $pendingTidalLogin.externalOpenError}
				<p class="error" role="alert">{$pendingTidalLogin.externalOpenError}</p>
				<input class="redirect-input" type="url" readonly value={$pendingTidalLogin.verifyUrl} aria-label="TIDAL sign-in URL" />
			{/if}
			<input
				class="redirect-input"
				type="url"
				bind:value={$pendingTidalLogin.redirectUrl}
				disabled={$pendingTidalLogin.phase !== 'awaiting'}
				aria-label="Final TIDAL redirect URL"
				placeholder="https://tidal.com/android/login/auth?code=..."
			/>
			{#if $pendingTidalLogin.error}
				<p class="error" role="alert">{$pendingTidalLogin.error}</p>
			{/if}
			<div class="actions">
				<button class="btn btn-ghost" onclick={pasteTidalRedirectUrl} disabled={$pendingTidalLogin.phase !== 'awaiting'}>Paste from clipboard</button>
				<button class="btn btn-primary" onclick={completeLogin} disabled={$pendingTidalLogin.phase !== 'awaiting' || !$pendingTidalLogin.redirectUrl.trim()}>{$pendingTidalLogin.phase === 'completing' ? 'Finishing login…' : 'Finish login'}</button>
				<button class="btn btn-ghost" onclick={cancelTidalLogin} disabled={$pendingTidalLogin.phase !== 'awaiting'}>Cancel login</button>
			</div>
			<p class="hint">
				Didn't open? <button type="button" class="hint-link" onclick={() => void openTidalVerifyUrl()} disabled={$pendingTidalLogin.phase !== 'awaiting'}>Open the page manually</button>.
			</p>
			{#if showSkip}
				<button class="btn btn-ghost" onclick={handleSkip} disabled={$pendingTidalLogin.phase !== 'awaiting'}>Skip for now</button>
			{/if}
		</div>
	{:else if connected}
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
	.btn-ghost:hover { background: var(--bg-hover); color: var(--text-primary); }
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
	.error { color: var(--state-error); margin: 0; }
	.success { color: var(--state-success); margin: 0; }
</style>
