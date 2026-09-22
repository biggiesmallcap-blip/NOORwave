<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { get } from 'svelte/store';
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import { api, getStoredToken, setStoredToken } from '$lib/api/client';
	import { remoteApi, RemoteRequestError, type PairingTicketResponse, type RemoteDevice, type RemoteStatus } from '$lib/api/remote';
	import { defaultPairingAddress, OperationGeneration, pairingAddressOptions, renderPairingQr } from '$lib/remote/qr';
	import { isTauri } from '$lib/util/external';
	import { isPlaying, playbackQueue } from '$lib/stores/player';
	import { connectWebSocket, disconnectWebSocket } from '$lib/api/ws';
	import { retainObservedStartupState, startupPresentation, type DesktopStartupState } from '$lib/desktop/startup';
	import { exposurePresentation, parseDesktopRemoteError, type DesktopRemoteState } from '$lib/desktop/remote';

	let status = $state<RemoteStatus | null>(null);
	let devices = $state<RemoteDevice[]>([]);
	let ticket = $state<PairingTicketResponse | null>(null);
	let qrDataUrl = $state('');
	let selectedAddress = $state<string | undefined>(undefined);
	let loading = $state(true);
	let busy = $state(false);
	let message = $state('');
	let error = $state('');
	let managementAvailable = $state(true);
	let startupState = $state<DesktopStartupState | null>(null);
	let startupBusy = $state(false);
	let tokenVisible = $state(false);
	let copied = $state(false);
	let expiresIn = $state(0);
	let poll: ReturnType<typeof setInterval> | null = null;
	let expiryTimer: ReturnType<typeof setInterval> | null = null;
	let controller: AbortController | null = null;
	let errorElement = $state<HTMLParagraphElement | null>(null);
	let statusLoadRunning = false;
	const pairingOperations = new OperationGeneration();
	let serverToken = $state('');
	let nativeState = $state<DesktopRemoteState | null>(null);
	let urlFallback = $state<HTMLInputElement | null>(null);

	let addresses = $derived(status ? pairingAddressOptions(status) : []);
	let startup = $derived(startupPresentation(startupState, startupBusy));
	let canPair = $derived(status?.state === 'running' && status.remote_assets_available && addresses.length > 0);
	let exposure = $derived(status ? exposurePresentation(status) : null);
	let selectedUrl = $derived(addresses.find((item) => item.id === selectedAddress)?.url ?? '');
	let selectedOption = $derived(addresses.find((item) => item.id === selectedAddress) ?? null);
	let connectionLabel = $derived(
		selectedOption?.friendly ? 'Recommended local address'
			: selectedOption?.recommended ? 'Recommended Wi-Fi address'
			: 'Alternate connection address'
	);
	let needsLocalRecovery = $derived(nativeState?.phase === 'failed' && nativeState.configured_host_mode === false);

	function discoveryMessage(nextStatus: RemoteStatus): string {
		const hostname = nextStatus.discovery.hostname ?? 'your local address';
		if (nextStatus.discovery.state === 'advertised') return `${hostname} is ready to use.`;
		if (nextStatus.discovery.state === 'starting') return `Preparing ${hostname}. Your Wi-Fi address works in the meantime.`;
		if (nextStatus.effective_host_mode) return `${hostname} is unavailable on this network. Use the Wi-Fi address below.`;
		return 'Enable phone remote to create a connection address.';
	}

	function troubleshootingSummary(nextStatus: RemoteStatus): string {
		const hostname = nextStatus.discovery.hostname ?? 'noor.local';
		if (nextStatus.discovery.state === 'advertised') return `NOORwave is listening and ${hostname} is ready to use.`;
		if (nextStatus.discovery.state === 'starting') return `NOORwave is listening. ${hostname} is still being prepared, so use the Wi-Fi address above for now.`;
		if (nextStatus.effective_host_mode) return `NOORwave is listening, but ${hostname} is not available on this network. Use the Wi-Fi address above.`;
		return 'Phone remote is not currently available on your network.';
	}

	function applyNativeState(next: DesktopRemoteState): void {
		nativeState = next;
		if (status) status = { ...status, configured_host_mode: next.configured_host_mode };
	}

	async function reportError(cause: unknown, fallback: string): Promise<void> {
		error = safeMessage(cause, fallback);
		await tick();
		errorElement?.focus();
	}

	function safeMessage(cause: unknown, fallback: string): string {
		return cause instanceof RemoteRequestError ? cause.message
			: cause instanceof Error ? cause.message
			: cause && typeof cause === 'object' && 'message' in cause && typeof cause.message === 'string' ? cause.message
			: fallback;
	}

	async function load(signal?: AbortSignal): Promise<void> {
		if (statusLoadRunning) return;
		statusLoadRunning = true;
		try {
			const [nextStatus, nextDevices] = await Promise.all([remoteApi.status(signal), remoteApi.devices(signal)]);
			status = nextStatus;
			devices = nextDevices.devices;
			managementAvailable = true;
			if (!selectedAddress || !pairingAddressOptions(nextStatus).some((item) => item.id === selectedAddress)) {
				selectedAddress = defaultPairingAddress(nextStatus);
			}
		} catch (cause) {
			if (signal?.aborted) return;
			if (cause instanceof RemoteRequestError && (cause.status === 401 || cause.status === 403)) {
				managementAvailable = false;
				await reportError(cause, 'Manage phone connections on the computer.');
			} else await reportError(cause, 'Could not load phone remote status.');
		} finally { loading = false; statusLoadRunning = false; }
	}

	async function loadStartup(): Promise<void> {
		if (!isTauri()) return;
		try { startupState = await invoke<DesktopStartupState>('get_startup_state'); }
		catch (cause) { await reportError(cause, 'Could not read start-at-sign-in state.'); }
	}

	onMount(() => {
		let disposed = false;
		let unlistenRemote: (() => void) | null = null;
		serverToken = getStoredToken() ?? '';
		controller = new AbortController();
		void load(controller.signal);
		void loadStartup();
		if (isTauri()) {
			void invoke<DesktopRemoteState>('get_remote_host_state').then(applyNativeState).catch(() => {});
			void listen<DesktopRemoteState>('remote-host-state-changed', (event) => applyNativeState(event.payload))
				.then((unlisten) => { if (disposed) unlisten(); else unlistenRemote = unlisten; })
				.catch(() => {});
		}
		poll = setInterval(() => void load(controller?.signal), 2000);
		return () => {
			disposed = true;
			unlistenRemote?.();
			controller?.abort();
			if (poll) clearInterval(poll);
			if (expiryTimer) clearInterval(expiryTimer);
			if (ticket) void remoteApi.cancelPairing(ticket.id).catch(() => {});
		};
	});

	async function changeHost(enabled: boolean): Promise<void> {
		const active = get(isPlaying);
		const queueCount = get(playbackQueue).length;
		const action = enabled ? 'Enable' : 'Disable';
		if (!confirm(`${action} phone remote and restart NOORwave's server? Playback is ${active ? 'active' : 'not active'} and ${queueCount} queued track${queueCount === 1 ? '' : 's'} will be cleared.`)) return;
		busy = true; error = ''; message = 'Restarting the local server…';
		try {
			await invoke('set_remote_host_mode', { enabled });
			message = enabled ? 'Phone remote is available while NOORwave is running.' : 'Phone remote is local-only.';
			await load();
		} catch (cause) {
			const nativeError = parseDesktopRemoteError(cause);
			if (nativeError) applyNativeState(nativeError.state);
			await reportError(nativeError ?? cause, 'The server restart failed.');
		}
		finally { busy = false; }
	}

	async function restartLocalOnly(): Promise<void> {
		busy = true; error = ''; message = 'Restarting the local-only server…';
		try {
			await invoke('restart_managed_server');
			message = 'Local-only service was restored. Network access remains disabled.';
			await load();
		} catch (cause) {
			const nativeError = parseDesktopRemoteError(cause);
			if (nativeError) applyNativeState(nativeError.state);
			await reportError(nativeError ?? cause, 'The local-only server could not be restarted.');
		} finally { busy = false; }
	}

	async function updateStartup(enabled: boolean): Promise<void> {
		if (!startupState) return;
		startupBusy = true; error = '';
		try { startupState = await invoke<DesktopStartupState>('set_start_at_login', { enabled }); }
		catch (cause) {
			startupState = retainObservedStartupState(startupState, cause);
			await reportError(cause, 'Start-at-sign-in could not be changed.');
		} finally { startupBusy = false; }
	}

	function updateExpiry(): void {
		expiresIn = ticket ? Math.max(0, Math.ceil((Date.parse(ticket.expires_at) - Date.now()) / 1000)) : 0;
	}

	async function createQr(address = selectedAddress): Promise<void> {
		const generation = pairingOperations.begin();
		busy = true; error = ''; message = '';
		try {
			if (ticket) await remoteApi.cancelPairing(ticket.id).catch(() => {});
			if (!pairingOperations.isCurrent(generation)) return;
			const created = await remoteApi.createPairing(address);
			if (!pairingOperations.isCurrent(generation)) { await remoteApi.cancelPairing(created.id).catch(() => {}); return; }
			const rendered = await renderPairingQr(created.pairing_url);
			if (!pairingOperations.isCurrent(generation)) { await remoteApi.cancelPairing(created.id).catch(() => {}); return; }
			ticket = created;
			qrDataUrl = rendered;
			updateExpiry();
			if (expiryTimer) clearInterval(expiryTimer);
			expiryTimer = setInterval(updateExpiry, 1000);
		} catch (cause) { if (pairingOperations.isCurrent(generation)) await reportError(cause, 'Could not create a pairing QR code.'); }
		finally { if (pairingOperations.isCurrent(generation)) busy = false; }
	}

	async function selectAddress(event: Event): Promise<void> {
		selectedAddress = (event.currentTarget as HTMLSelectElement).value;
		if (ticket) await createQr(selectedAddress);
	}

	async function closeQr(): Promise<void> {
		pairingOperations.invalidate();
		busy = false;
		const old = ticket;
		ticket = null; qrDataUrl = ''; expiresIn = 0;
		if (expiryTimer) clearInterval(expiryTimer);
		expiryTimer = null;
		if (old) await remoteApi.cancelPairing(old.id).catch(() => {});
	}

	async function copyText(value: string, fallback?: HTMLInputElement | null): Promise<void> {
		try { await navigator.clipboard.writeText(value); copied = true; setTimeout(() => copied = false, 1800); }
		catch {
			message = 'Clipboard permission is unavailable. The address is selected for manual copy.';
			await tick();
			const target = fallback ?? urlFallback;
			target?.focus();
			target?.select();
		}
	}

	async function renameDevice(device: RemoteDevice): Promise<void> {
		const name = prompt('Name this phone', device.name)?.trim();
		if (!name || name === device.name) return;
		try { await remoteApi.renameDevice(device.id, name); await load(); }
		catch (cause) { await reportError(cause, 'Could not rename this phone.'); }
	}

	async function revokeDevice(device: RemoteDevice): Promise<void> {
		if (!confirm(`Disconnect ${device.name}? It will need a new QR code or the shared PIN.`)) return;
		try { await remoteApi.revokeDevice(device.id); await load(); }
		catch (cause) { await reportError(cause, 'Could not disconnect this phone.'); }
	}

	async function resetAll(): Promise<void> {
		if (!confirm(`Reset all remote access? ${devices.length} paired device${devices.length === 1 ? '' : 's'} will be disconnected, the shared PIN will change, and every open remote session will close.`)) return;
		busy = true; error = '';
		disconnectWebSocket();
		try {
			const result = await api.regenerateServerToken();
			setStoredToken(result.token);
			serverToken = result.token;
			connectWebSocket();
			await closeQr(); await load();
			message = 'All remote access was reset. Use the new PIN or create a new QR code.';
		} catch (cause) {
			connectWebSocket();
			await reportError(cause, 'Remote access could not be reset.');
		}
		finally { busy = false; }
	}
</script>

<section data-setting-id="phone-remote" class="glass-panel section-panel phone-remote-panel" aria-busy={loading || busy}>
	<SectionHeader eyebrow="Connection" title="Phone Remote" subtitle="Connect a phone on the same trusted Wi-Fi network." />

	{#if error}<p bind:this={errorElement} class="remote-alert error" role="alert" tabindex="-1">{error}</p>{/if}
	{#if message}<p class="remote-alert" role="status" aria-live="polite">{message}</p>{/if}

	{#if !managementAvailable}
		<p class="remote-copy">Open Settings in the NOORwave desktop app to manage hosting, QR codes, and paired devices.</p>
	{:else if status}
		<div class="status-line">
			<span class:online={exposure?.online} aria-hidden="true"></span>
			<div><strong>{exposure?.label}</strong><small>{discoveryMessage(status)}</small></div>
		</div>

		<div class="setting-row">
			<div><strong>Make phone remote available whenever NOORwave is running</strong><p>Changing this restarts the server and clears playback and the current queue.</p></div>
			<Toggle checked={nativeState?.configured_host_mode ?? status.configured_host_mode} disabled={busy || status.control !== 'desktop' || !isTauri()} label="Make phone remote available whenever NOORwave is running" onchange={(event) => void changeHost(event.currentTarget.checked)} />
		</div>
		{#if needsLocalRecovery}
			<div class="recovery-block"><p>Network access is disabled, but the local server did not come back. Restart it locally; this does not enable LAN access.</p><button class="btn btn-primary" type="button" disabled={busy} onclick={() => void restartLocalOnly()}>Restart local-only server</button></div>
		{/if}

		<div class="setting-row">
			<div><strong>Start NOORwave in the tray when I sign in</strong><p>{startup.message}</p></div>
			<Toggle checked={startup.checked} disabled={startup.disabled} label="Start NOORwave in the tray when I sign in" onchange={(event) => void updateStartup(event.currentTarget.checked)} />
		</div>

		{#if status.restart_required}<p class="remote-alert">The standalone server preference is saved. Restart the server process to apply it.</p>{/if}

		<div class="pairing-block">
			<div class="pairing-intro"><span class="step-label">Step 1</span><div><strong>Pair your phone</strong><p>Use the address below, then scan a QR. The QR and temporary code expire after two minutes and work once; no permanent PIN is shared.</p></div></div>
			{#if selectedOption}
				<div class="connection-card">
					<div class="connection-card-heading"><span>{connectionLabel}</span>{#if selectedOption.friendly}<span class="local-badge">Local discovery</span>{/if}</div>
					<input bind:this={urlFallback} id="remote-url-fallback" class="url-fallback" type="text" readonly value={selectedUrl} aria-label={connectionLabel} onclick={(event) => event.currentTarget.select()} />
					<div class="connection-actions"><button class="btn btn-glass" type="button" onclick={() => void copyText(selectedUrl, urlFallback)}>{copied ? 'Copied address' : 'Copy address'}</button><span>{selectedOption.friendly ? 'Best for bookmarks and returning later.' : 'Use this while local discovery is unavailable.'}</span></div>
				</div>
			{/if}
			{#if ticket && qrDataUrl}
				<div class="qr-wrap">
					<div class="qr-header">
						<div><strong>Scan to pair</strong><p>Open your iPhone camera and point it at the code.</p></div>
						<span class="expiry-badge" role="timer">{expiresIn}s</span>
					</div>
					<div class="qr-stage">
						<div class="qr-code-frame"><img src={qrDataUrl} alt="Pair this phone with NOORwave" width="320" height="320" /></div>
					</div>
					<div class="pairing-alternative">
						<p>Already installed on iPhone? Open the NOORwave app and enter:</p>
						<code class="pairing-code">{ticket.pairing_code.slice(0, 3)} {ticket.pairing_code.slice(3)}</code>
					</div>
					<code class="pairing-url">{ticket.pairing_url.replace(/#pair=.*/, '#pair=…')}</code>
					<div class="actions"><button class="btn btn-primary" type="button" disabled={busy} onclick={() => void createQr()}>Refresh QR</button><button class="btn btn-glass" type="button" disabled={busy} onclick={() => void closeQr()}>Cancel</button></div>
				</div>
			{:else}
				<button class="btn btn-primary touch" type="button" disabled={busy || !canPair} onclick={() => void createQr()}>Show pairing QR</button>
			{/if}
			{#if addresses.length > 1}
				<details class="address-options">
					<summary>Use a different connection address</summary>
					<p>Choose a VPN or virtual address only when the phone is connected to that same network.</p>
					<label for="remote-address">Connection address</label>
					<select id="remote-address" value={selectedAddress} onchange={(event) => void selectAddress(event)} disabled={busy || !canPair}>
						{#each addresses as address (address.id)}<option value={address.id}>{address.label} — {address.url}</option>{/each}
					</select>
				</details>
			{/if}
		</div>

		<details class="manual" data-setting-id="access-pin">
			<summary>Recovery: use the master PIN</summary>
			<p>The PIN is an optional fallback for browsers that cannot pair. PIN sessions are shared and cannot be individually listed or revoked; prefer the QR or temporary code for normal use.</p>
			<div class="pin-row"><code>{tokenVisible ? serverToken : '••••••'}</code><button class="btn btn-glass" type="button" onclick={() => tokenVisible = !tokenVisible}>{tokenVisible ? 'Hide PIN' : 'Show PIN'}</button><button class="btn btn-glass" type="button" onclick={() => void copyText(serverToken)}>Copy PIN</button></div>
			<button class="btn btn-danger" type="button" disabled={busy} onclick={() => void resetAll()}>Reset all remote access</button>
		</details>

		<div class="devices">
			<h3>Paired devices</h3>
			{#if devices.length === 0}<p>No individually paired phones yet.</p>{/if}
			{#each devices as device (device.id)}
				<div class="device-row"><div><strong>{device.name}</strong><small>Paired {new Date(device.paired_at).toLocaleString()} · {device.last_seen_at ? `Last seen ${new Date(device.last_seen_at).toLocaleString()}` : 'Not seen yet'}</small></div><div class="actions"><button class="btn btn-glass" type="button" onclick={() => void renameDevice(device)}>Rename</button><button class="btn btn-glass" type="button" onclick={() => void revokeDevice(device)}>Revoke</button></div></div>
			{/each}
		</div>

		<details class="diagnostics">
			<summary>Troubleshooting and diagnostics</summary>
			<p class="diagnostic-summary">{troubleshootingSummary(status)}</p>
			<ul class="troubleshooting-list"><li>Keep both devices on the same trusted Wi-Fi network.</li><li>Allow NOORwave on a private network in Windows Firewall if prompted.</li><li>Guest Wi-Fi/client isolation and VPNs may block local devices.</li><li>If <code>{status.discovery.hostname ?? 'noor.local'}</code> does not open, use the Wi-Fi address above and refresh the QR.</li></ul>
			{#each status.diagnostics as diagnostic}<p class="diagnostic-note">{diagnostic.message}</p>{/each}
		</details>
	{/if}
</section>

<style>
	.phone-remote-panel { display: flex; flex-direction: column; gap: 18px; padding: 24px; }
	.remote-alert { padding: 10px 12px; border-radius: 8px; background: var(--accent-soft); margin: 0; }
	.remote-alert.error { color: var(--state-error); border: 1px solid color-mix(in srgb, var(--state-error) 40%, transparent); }
	.status-line { display: flex; align-items: flex-start; gap: 10px; padding: 4px 0 16px; border-bottom: 1px solid var(--border-subtle); }
	.status-line > span { width: 10px; height: 10px; margin-top: 6px; border-radius: 50%; background: var(--text-tertiary); box-shadow: 0 0 0 4px color-mix(in srgb, var(--text-tertiary) 12%, transparent); }
	.status-line > span.online { background: var(--state-success); box-shadow: 0 0 0 4px color-mix(in srgb, var(--state-success) 14%, transparent); }
	.status-line strong, .status-line small { display: block; }
	.status-line small { color: var(--text-secondary); margin-top: 3px; }
	.setting-row, .device-row { display: flex; align-items: center; justify-content: space-between; gap: 18px; padding: 14px 0; border-top: 1px solid var(--border-subtle); }
	.setting-row p, .pairing-block p, .manual p, .devices p, .diagnostics p { color: var(--text-secondary); margin: 4px 0 0; line-height: var(--line-height-normal); }
	.pairing-block, .devices { display: flex; flex-direction: column; gap: 12px; padding-top: 18px; border-top: 1px solid var(--border-subtle); }
	.pairing-intro { display: flex; align-items: flex-start; gap: 10px; }
	.pairing-intro p { max-width: 680px; }
	.step-label { flex: 0 0 auto; padding: 4px 7px; border: 1px solid var(--accent-line); border-radius: 999px; background: var(--accent-soft); color: var(--accent-strong); font-size: var(--font-size-xs); font-weight: var(--font-weight-bold); letter-spacing: .05em; text-transform: uppercase; }
	.connection-card { display: grid; gap: 10px; padding: 15px; border: 1px solid color-mix(in srgb, var(--accent-line) 68%, var(--border-subtle)); border-radius: var(--radius-md); background: linear-gradient(130deg, color-mix(in srgb, var(--accent-soft) 55%, transparent), transparent 62%), var(--bg-surface); }
	.connection-card-heading, .connection-actions { display: flex; align-items: center; justify-content: space-between; gap: 10px; flex-wrap: wrap; }
	.connection-card-heading > span:first-child { color: var(--text-secondary); font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); }
	.local-badge { padding: 3px 7px; border-radius: 999px; background: color-mix(in srgb, var(--state-success) 15%, transparent); color: var(--state-success); font-size: var(--font-size-xs); font-weight: var(--font-weight-semibold); }
	.connection-actions > span { color: var(--text-tertiary); font-size: var(--font-size-sm); }
	select { width: 100%; min-height: 44px; border: 1px solid var(--border-subtle); border-radius: 8px; background: var(--bg-surface); color: var(--text-primary); padding: 8px 10px; }
	.url-fallback { width: 100%; min-height: 44px; border: 1px solid var(--border-subtle); border-radius: 8px; background: color-mix(in srgb, var(--bg-base) 58%, var(--bg-surface)); color: var(--text-primary); padding: 8px 10px; font-family: var(--font-mono, monospace); }
	.recovery-block { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 12px; border: 1px solid color-mix(in srgb, var(--state-error) 35%, transparent); border-radius: 8px; }
	.qr-wrap {
		display: grid;
		gap: 14px;
		padding: 18px;
		color: var(--text-primary);
		background:
			linear-gradient(145deg, color-mix(in srgb, var(--accent-soft) 35%, transparent), transparent 45%),
			color-mix(in srgb, var(--instrument-surface) 82%, var(--panel-bg));
		border: 1px solid color-mix(in srgb, var(--panel-border) 72%, var(--instrument-border));
		border-radius: var(--radius-lg);
		box-shadow: inset 0 1px 0 color-mix(in srgb, var(--instrument-edge) 44%, transparent), var(--panel-shadow);
	}
	.qr-header { display: flex; align-items: flex-start; justify-content: space-between; gap: 16px; }
	.qr-header p { margin-top: 3px; }
	.expiry-badge { flex: 0 0 auto; min-width: 58px; padding: 7px 10px; border: 1px solid var(--accent-line); border-radius: 999px; background: var(--accent-soft); color: var(--accent-strong); font-family: var(--font-mono, monospace); text-align: center; }
	.qr-stage { display: grid; place-items: center; padding: 20px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: radial-gradient(circle at 50% 32%, var(--surface-2), var(--bg-surface) 68%); }
	.qr-code-frame { width: min(100%, 320px); padding: 12px; border-radius: var(--radius-md); background: #fff; box-shadow: 0 14px 36px rgba(0, 0, 0, .28); }
	.qr-wrap img { display: block; width: 100%; height: auto; image-rendering: pixelated; }
	.qr-wrap code { max-width: 100%; overflow-wrap: anywhere; }
	.pairing-alternative { display: grid; justify-items: center; gap: 8px; padding: 13px 16px; border: 1px solid var(--accent-line); border-radius: var(--radius-md); background: color-mix(in srgb, var(--accent-soft) 68%, transparent); text-align: center; }
	.pairing-alternative p { margin: 0; }
	.qr-wrap .pairing-code { color: var(--accent-strong); font-size: var(--font-size-xl); font-weight: var(--font-weight-semibold); letter-spacing: .14em; }
	.pairing-url { justify-self: center; padding: 6px 9px; border-radius: var(--radius-xs); background: var(--bg-surface); color: var(--text-tertiary); text-align: center; }
	.qr-wrap > .actions { justify-content: center; }
	.actions, .pin-row { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
	.touch, summary, .device-row button { min-height: 44px; }
	.manual, .diagnostics, .address-options { padding-top: 14px; border-top: 1px solid var(--border-subtle); }
	.address-options { display: grid; gap: 10px; }
	.address-options p { margin: 0; color: var(--text-secondary); }
	.diagnostic-summary { color: var(--text-primary) !important; }
	.troubleshooting-list { display: grid; gap: 6px; margin: 12px 0 0; padding-left: 20px; color: var(--text-secondary); }
	.troubleshooting-list code { color: var(--accent-strong); font-family: var(--font-mono, monospace); }
	.diagnostic-note { margin-top: 12px !important; padding: 9px 11px; border-left: 2px solid var(--accent-line); background: color-mix(in srgb, var(--accent-soft) 34%, transparent); }
	summary { display: flex; align-items: center; cursor: pointer; font-weight: var(--font-weight-bold); }
	.pin-row { margin: 12px 0; }
	.pin-row code { font-size: var(--font-size-xl); letter-spacing: .18em; }
	.device-row small { display: block; margin-top: 4px; color: var(--text-tertiary); }
	.btn-danger { color: var(--state-error); }
	@media (max-width: 560px) {
		.setting-row, .device-row { align-items: flex-start; }
		.device-row { flex-direction: column; }
		.actions, .actions .btn, .pin-row .btn { min-height: 44px; }
		.connection-actions { align-items: flex-start; }
		.connection-actions > span { width: 100%; }
		.qr-wrap { padding: 14px; }
		.qr-stage { padding: 14px; }
	}
</style>
