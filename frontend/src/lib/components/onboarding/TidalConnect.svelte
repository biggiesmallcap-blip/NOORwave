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
				<h2 class="onboarding-title">Connect TIDAL</h2>
				<p class="onboarding-lede">NOORwave plays from your TIDAL library. Sign in once and we'll keep your tracks in sync.</p>
			{/if}
			{#if errorMsg}
				<p class="error" role="alert">{errorMsg}</p>
			{/if}
			<div class="actions" class:onboarding-actions={variant === 'onboarding'}>
				<button class="btn btn-primary" class:onboarding-action={variant === 'onboarding'} onclick={start}>
					{errorMsg ? 'Try again' : 'Connect TIDAL'}
				</button>
				{#if showSkip}
					<button class="btn btn-glass" class:onboarding-action={variant === 'onboarding'} onclick={handleSkip}>Skip for now</button>
				{/if}
			</div>
		</div>
	{:else if $pendingTidalLogin?.phase === 'starting'}
		<p class="muted">Opening TIDAL sign-in...</p>
	{:else if $pendingTidalLogin}
		<div class="redirect-login">
			{#if variant === 'onboarding'}<h2 class="onboarding-title">Connect TIDAL</h2>{/if}
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
			<div class="actions" class:onboarding-actions={variant === 'onboarding'}>
				<button class="btn btn-glass" class:onboarding-action={variant === 'onboarding'} onclick={pasteTidalRedirectUrl} disabled={$pendingTidalLogin.phase !== 'awaiting'}>Paste from clipboard</button>
				<button class="btn btn-primary" class:onboarding-action={variant === 'onboarding'} onclick={completeLogin} disabled={$pendingTidalLogin.phase !== 'awaiting' || !$pendingTidalLogin.redirectUrl.trim()}>{$pendingTidalLogin.phase === 'completing' ? 'Finishing login…' : 'Finish login'}</button>
				<button class="btn btn-glass" class:onboarding-action={variant === 'onboarding'} onclick={cancelTidalLogin} disabled={$pendingTidalLogin.phase !== 'awaiting'}>Cancel login</button>
			</div>
			<p class="hint">
				Didn't open? <button type="button" class="hint-link" onclick={() => void openTidalVerifyUrl()} disabled={$pendingTidalLogin.phase !== 'awaiting'}>Open the page manually</button>.
			</p>
			{#if showSkip}
				<button class="btn btn-glass" class:onboarding-action={variant === 'onboarding'} onclick={handleSkip} disabled={$pendingTidalLogin.phase !== 'awaiting'}>Skip for now</button>
			{/if}
		</div>
	{:else if connected}
		<p class="success">TIDAL connected.</p>
	{/if}
</div>

<style>
	.tidal-connect { display: flex; flex-direction: column; gap: var(--space-4); width: 100%; }
	.variant-onboarding { text-align: center; align-items: center; }
	.prompt { display: flex; flex-direction: column; gap: var(--space-4); }
	.variant-onboarding .prompt { align-items: center; gap: var(--space-5); width: 100%; }
	.actions { display: flex; gap: var(--space-3); justify-content: center; flex-wrap: wrap; }
	.redirect-login { display: flex; flex-direction: column; align-items: center; gap: var(--space-4); width: min(100%, 48ch); }
	.variant-onboarding .redirect-login { gap: var(--space-5); }
	.redirect-input {
		width: 100%;
		min-height: var(--settings-control-height, 44px);
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--border-muted);
		border-radius: var(--radius-sm);
		background: var(--bg-elevated);
		color: var(--text-primary);
		font: inherit;
		font-size: var(--font-size-sm);
	}
	.hint, .muted { color: var(--text-secondary); margin: 0; font-size: var(--font-size-sm); line-height: var(--line-height-normal); }
	.hint-link {
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		color: var(--accent-strong);
		cursor: pointer;
		text-decoration: underline;
		text-underline-offset: 3px;
	}
	.error { color: var(--state-error); margin: 0; font-size: var(--font-size-sm); }
	.success { color: var(--state-success); margin: 0; }
</style>
