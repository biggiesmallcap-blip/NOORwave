import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const source = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'PhoneRemotePanel.svelte'), 'utf8');

describe('Phone Remote panel accessibility contract', () => {
	it('announces and focuses asynchronous errors', () => {
		expect(source).toContain('role="alert"');
		expect(source).toContain('tabindex="-1"');
		expect(source).toContain('errorElement?.focus()');
	});

	it('keeps phone actions touch-sized at narrow widths and QR alternatives visible', () => {
		expect(source).toContain('@media (max-width: 560px)');
		expect(source).toContain('min-height: 44px');
		expect(source).toContain('alt="Pair this phone with NOORwave"');
		expect(source).toContain('Copy address');
		expect(source).toContain('readonly');
		expect(source).toContain('target?.select()');
	});

	it('shows a user-facing address and keeps raw listener details in diagnostics', () => {
		expect(source).toContain('Recommended local address');
		expect(source).toContain('Use a different connection address');
		expect(source).not.toContain('<small>{status.bind_address}</small>');
		expect(source).toContain('troubleshootingSummary(status)');
		expect(source).toContain("status.discovery.hostname ?? 'noor.local'");
		expect(source).not.toContain('DISCOVERY_STARTING');
	});

	it('serializes QR controls while creation is in flight', () => {
		expect(source).toContain('disabled={busy} onclick={() => void createQr()}');
		expect(source).toContain('disabled={busy} onclick={() => void closeQr()}');
		expect(source).toContain('OperationGeneration');
	});

	it('shows a one-use code for an already-installed iPhone PWA', () => {
		expect(source).toContain('Open the installed NOORwave app');
		expect(source).toContain('ticket.pairing_code.slice(0, 3)');
		expect(source).toContain('temporary code expire after two minutes and work once');
	});

	it('keeps the scanner quiet zone inside a themed NOORwave pairing card', () => {
		expect(source).toContain('class="qr-stage"');
		expect(source).toContain('class="qr-code-frame"');
		expect(source).toContain('background: #fff');
		expect(source).toContain('color: var(--text-primary)');
		expect(source).toContain('var(--instrument-surface)');
	});

	it('reconciles native transition events and offers fail-closed local recovery', () => {
		expect(source).toContain("listen<DesktopRemoteState>('remote-host-state-changed'");
		expect(source).toContain("invoke('restart_managed_server')");
		expect(source).toContain('Restart local-only server');
	});

	it('confirms inline instead of with native browser dialogs', () => {
		expect(source).not.toMatch(/\b(confirm|prompt)\(/);
		expect(source).toContain('class="inline-confirm"');
		expect(source).toContain('Restart now');
		expect(source).toContain('class="device-name-input"');
	});

	it('closes the QR when the phone redeems it and offers a new code on expiry', () => {
		expect(source).toContain("nextStatus.ticket.state === 'redeemed'");
		expect(source).toContain('is paired. It reconnects on its own');
		expect(source).toContain('This code expired.');
		expect(source).toContain('ticket ? 1000 : 3000');
	});

	it('shows live connection state per paired device', () => {
		expect(source).toContain('class:online={device.connected}');
		expect(source).toContain('Connected now');
	});

	it('keeps PIN sign-in opt-in, rotatable, and swaps this window onto a new PIN', () => {
		expect(source).toContain('label="PIN sign-in"');
		expect(source).toContain('remoteApi.setPinAccess(enabled)');
		expect(source).toContain('remoteApi.rotatePin()');
		expect(source).toContain('{#if pinAccess && status.pin_access}');
		const swap = source.slice(source.indexOf('async function swapPin'), source.indexOf('async function rotatePin'));
		expect(swap).toContain('disconnectWebSocket()');
		expect(swap).toContain('setStoredToken(result.token)');
		expect(swap).toContain('connectWebSocket()');
	});
});
