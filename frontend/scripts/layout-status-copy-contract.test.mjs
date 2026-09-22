import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(import.meta.dirname, '../src/routes/+layout.svelte'), 'utf8');
const settingsSource = readFileSync(resolve(import.meta.dirname, '../src/routes/settings/+page.svelte'), 'utf8');
const traySource = readFileSync(resolve(import.meta.dirname, '../../noor-app/src/tray.rs'), 'utf8');

describe('layout status copy', () => {
	test('sidebar connection status uses plain websocket meaning, said once', () => {
		expect(source).toContain("'Connected'");
		expect(source).toContain("'Offline'");
		expect(source).not.toContain('Observatory live');
		expect(source).not.toContain('Realtime stream is locked in');
		expect(source).not.toContain('Waiting for websocket relay');
		// The pill used to state the same fact three ways (dot, headline, and a
		// "Realtime updates active" subtitle). The dot plus one headline is it.
		expect(source).not.toContain('Realtime updates active');
		expect(source).not.toContain('Waiting for realtime updates');
	});

	test('shuffle sidebar status is labeled as state, not a standalone notification', () => {
		expect(source).toContain('Shuffle: ${shuffleStatusLabels[$shuffleMode]}');
		expect(source).toContain("true: 'True random'");
		expect(source).not.toContain('<p class="status-line">{shuffleLabels[$shuffleMode]}</p>');
	});

	test('sidebar version badge calls out available patches', () => {
		expect(source).toContain("pendingDesktopUpdate = await invoke<DesktopUpdateInfo | null>('get_update_state')");
		expect(source).toContain('pendingDesktopUpdate = event.payload;');
		expect(source).toContain('class="live-version patch-available"');
		expect(source).toContain('View patch v${updateAvailableVersion}');
		expect(source).toContain('onclick={() => void openPatchInfo()}');
		expect(source).toContain('class="patch-info-dialog glass-panel"');
		expect(source).toContain('The engineers insist this version is better. Update?');
		expect(source).toContain('Trust the Engineers');
		expect(source).toContain('I Know Better');
		expect(source).not.toContain('window.confirm(');
		expect(source).toContain("await invoke('install_pending_update');");
	});

	test('settings and tray route updates through the patch info dialog', () => {
		expect(settingsSource).toContain('Patch info');
		expect(settingsSource).toContain("await emit('open-update-details');");
		expect(traySource).toContain('available - view patch info');
		expect(traySource).toContain('handle.emit("open-update-details", ())');
	});
});
