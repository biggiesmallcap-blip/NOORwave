import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '+layout.svelte'), 'utf8');

describe('layout auth gate contract', () => {
	test('protected app and remote shells do not mount before auth is ready', () => {
		const onboardingRoute = source.indexOf('{#if isOnboardingRoute}');
		const authGate = source.indexOf('{:else if !authReady}');
		const onboardingGate = source.indexOf('{:else if !onboardingChecked}');
		const remoteShell = source.indexOf('{:else if isRemoteRoute}');
		const appShell = source.indexOf('<div class="app-shell"');

		expect(onboardingRoute).toBeGreaterThanOrEqual(0);
		expect(authGate).toBeGreaterThan(onboardingRoute);
		expect(onboardingGate).toBeGreaterThan(authGate);
		expect(remoteShell).toBeGreaterThan(onboardingGate);
		expect(appShell).toBeGreaterThan(remoteShell);
	});
});
