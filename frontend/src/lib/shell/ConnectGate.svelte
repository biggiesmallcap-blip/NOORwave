<!--
	Manual sign-in for a phone or browser that is not paired yet: a temporary
	pairing code, or the master PIN when the listener allows PIN sign-in. The
	layout runs the automatic bootstrap and decides when this gate shows.
-->
<script lang="ts">
	import { getApiBase } from '$lib/api/client';
	import { remoteApi, RemoteRequestError } from '$lib/api/remote';
	import { manualPinResponseError, storePairedSession } from '$lib/remote/connection';
	import { currentDeviceName } from '$lib/remote/device_name';
	import { clearSessionToken, setSessionToken } from '$lib/remote/session_token';
	import { showToast } from '$lib/stores/toast';

	interface Props {
		/** Shown under the pad; the bootstrap writes its own failures here too. */
		error: string;
		bootstrapBusy: boolean;
		networkUnavailable: boolean;
		onretry: () => void;
		onconnected: () => void;
	}

	let { error = $bindable(), bootstrapBusy, networkUnavailable, onretry, onconnected }: Props = $props();

	let connectTokenInput = $state('');
	let connectMethod = $state<'pairing' | 'pin'>('pairing');
	let submitting = $state(false);
	let busy = $derived(bootstrapBusy || submitting);
	let pinInputEl = $state<HTMLInputElement | null>(null);
	let pinLoginAvailable = $state(false);

	// The server refuses the shared PIN from other devices unless the listener
	// turned PIN sign-in on, so only offer it when it can work.
	$effect(() => {
		remoteApi.identity().then((identity) => {
			pinLoginAvailable = identity.pin_login === true;
			if (!pinLoginAvailable && connectMethod === 'pin') { connectMethod = 'pairing'; connectTokenInput = ''; }
		}).catch(() => {});
	});

	$effect(() => {
		pinInputEl?.focus();
	});

	function handlePinInput(event: Event) {
		const el = event.target as HTMLInputElement;
		const code = el.value.replace(/\D/g, '').slice(0, 6);
		connectTokenInput = code;
		el.value = code;
		error = '';
	}

	function focusPin() {
		pinInputEl?.focus();
	}

	function switchMethod() {
		connectMethod = connectMethod === 'pairing' ? 'pin' : 'pairing';
		connectTokenInput = '';
		error = '';
		setTimeout(focusPin, 0);
	}

	async function submitConnect() {
		error = '';
		const t = connectTokenInput.trim();
		const valid = /^\d{6}$/.test(t);
		if (!valid) { error = 'Enter all 6 digits.'; return; }
		submitting = true;
		try {
			if (connectMethod === 'pairing') {
				const paired = await remoteApi.redeem(t, currentDeviceName());
				const remembered = storePairedSession(paired);
				if (!remembered) showToast('Connected for this session, but this phone could not save the connection.', 'success', 8000);
				onconnected();
				return;
			}
			const resp = await fetch(`${getApiBase()}/api/status`, {
				headers: { authorization: `Bearer ${t}` }
			});
			const responseError = manualPinResponseError(resp.status);
			if (responseError) {
				error = responseError;
				if (resp.status === 401 || resp.status === 403) connectTokenInput = '';
				setTimeout(focusPin, 0);
				return;
			}
			setSessionToken(t);
			onconnected();
		} catch (err) {
			clearSessionToken();
			if (err instanceof RemoteRequestError && err.detail.error === 'PAIRING_INVALID') error = 'Temporary code expired or was already used. Create a new one on the computer.';
			else if (err instanceof RemoteRequestError && err.status === 429) error = 'Too many attempts. Wait a moment and create a new code.';
			else error = 'Connection failed. Is the server running?';
		} finally {
			submitting = false;
		}
	}
</script>

<div class="connect-backdrop">
	<div class="connect-panel glass-panel">
		<div class="connect-brand">
			<span class="connect-brand-mark">
				<img src="/noor-icon-transparent.svg" alt="" aria-hidden="true" />
			</span>
			<span class="connect-brand-name">NOOR</span>
		</div>
		<h2 class="connect-title">Connect to NOORwave</h2>
		<p class="connect-copy">
			{connectMethod === 'pairing'
				? 'Enter the temporary 6-digit code shown beside the QR.'
				: 'Enter the master recovery PIN from the computer settings.'}
		</p>

		<button type="button" class="pin-pad" onclick={focusPin} aria-label="Pairing code or PIN input">
			{#each [0,1,2,3,4,5] as i}
				<span
					class="pin-digit"
					class:filled={i < connectTokenInput.length}
					class:active={i === connectTokenInput.length && !busy}
				>
					{connectTokenInput[i] ?? ''}
				</span>
			{/each}
		</button>

		<input
			bind:this={pinInputEl}
			class="pin-hidden-input"
			inputmode="numeric"
			pattern="[0-9]*"
			maxlength="6"
			autocomplete="one-time-code"
			value={connectTokenInput}
			oninput={handlePinInput}
			onkeydown={(e) => e.key === 'Enter' && void submitConnect()}
			disabled={busy}
			aria-label={connectMethod === 'pairing' ? '6-digit temporary pairing code' : '6-digit master recovery PIN'}
		/>
		<button class="btn btn-primary" type="button" disabled={busy || !/^\d{6}$/.test(connectTokenInput)} onclick={() => void submitConnect()}>Connect</button>
		{#if pinLoginAvailable || connectMethod === 'pin'}
			<button class="btn btn-glass" type="button" disabled={busy} onclick={switchMethod}>
				{connectMethod === 'pairing' ? 'Use master PIN instead' : 'Use temporary code instead'}
			</button>
		{/if}

		{#if error}
			<p class="connect-error" role="alert" aria-live="assertive">{error}</p>
		{/if}
		{#if networkUnavailable}
			<button class="btn btn-primary" type="button" disabled={busy} onclick={onretry}>Retry connection</button>
		{/if}
		{#if busy}
			<p class="connect-copy">Connecting…</p>
		{/if}
	</div>
</div>

<style>
	.connect-backdrop {
		position: fixed;
		inset: 0;
		z-index: var(--z-tooltip);
		background: var(--bg-base, #0d0d12);
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 24px;
	}

	.connect-panel {
		width: 100%;
		max-width: 400px;
		display: flex;
		flex-direction: column;
		gap: 16px;
		padding: 32px;
		border-radius: var(--radius-lg, 16px);
	}

	.connect-brand {
		display: flex;
		align-items: center;
		gap: 10px;
		margin-bottom: 4px;
	}

	.connect-brand-mark {
		width: 36px;
		height: 36px;
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.connect-brand-mark img {
		width: 100%;
		height: 100%;
		object-fit: contain;
	}

	.connect-brand-name {
		font-size: var(--font-size-lg);
		font-weight: 800;
		letter-spacing: 0.12em;
		color: var(--text-primary);
	}

	.connect-title {
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		color: var(--text-primary);
	}

	.connect-copy {
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		line-height: var(--line-height-normal);
	}

	.pin-pad {
		display: flex;
		gap: 10px;
		justify-content: center;
		margin: 8px 0 4px;
		background: none;
		border: none;
		padding: 0;
		cursor: text;
	}

	.pin-digit {
		flex: 0 0 auto;
		width: 44px;
		height: 56px;
		border-radius: var(--radius-sm, 8px);
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.1);
		display: flex;
		align-items: center;
		justify-content: center;
		font-family: var(--font-mono, monospace);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
		transition: border-color var(--motion-fast), background var(--motion-fast);
	}

	.pin-digit.filled {
		background: var(--accent-soft);
		border-color: var(--accent-line);
	}

	.pin-digit.active {
		border-color: var(--accent);
		box-shadow: 0 0 0 3px var(--accent-soft);
	}

	.pin-hidden-input {
		position: absolute;
		opacity: 0;
		pointer-events: none;
		width: 1px;
		height: 1px;
	}

	.connect-error {
		font-size: var(--font-size-sm);
		color: #ffb0b0;
		text-align: center;
	}

	@media (max-width: 420px) {
		.pin-digit {
			width: 40px;
			height: 52px;
			font-size: var(--font-size-xl);
		}
		.pin-pad { gap: 8px; }
	}
</style>
