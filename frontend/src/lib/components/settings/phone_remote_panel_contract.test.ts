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
		expect(source).toContain('Copy selected address');
		expect(source).toContain('readonly');
		expect(source).toContain('Connection URL');
		expect(source).toContain('target?.select()');
	});

	it('serializes QR controls while creation is in flight', () => {
		expect(source).toContain('disabled={busy} onclick={() => void createQr()}');
		expect(source).toContain('disabled={busy} onclick={() => void closeQr()}');
		expect(source).toContain('OperationGeneration');
	});

	it('shows a one-use code for an already-installed iPhone PWA', () => {
		expect(source).toContain('Already installed on iPhone?');
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
});
