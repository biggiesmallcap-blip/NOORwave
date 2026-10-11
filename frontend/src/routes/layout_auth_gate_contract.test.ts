import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '+layout.svelte'), 'utf8');
const gate = readFileSync(join(here, '..', 'lib', 'shell', 'ConnectGate.svelte'), 'utf8');

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

	test('an installed iPhone PWA can redeem a temporary pairing code without the master PIN', () => {
		expect(source).toContain('<ConnectGate');
		expect(source).toContain('remoteApi.redeem(ticket, currentDeviceName(), signal)');
		expect(gate).toContain('temporary 6-digit code');
		expect(gate).toContain("connectMethod === 'pairing'");
		expect(gate).toContain('await remoteApi.redeem(t, currentDeviceName())');
		expect(gate).toContain('storePairedSession(paired)');
		expect(gate).toContain('Use master PIN instead');
		expect(gate).toContain('pinLoginAvailable = identity.pin_login === true');
		expect(gate).toContain("{#if pinLoginAvailable || connectMethod === 'pin'}");
	});

	test('revalidates a paired device before deleting its credential after a rejected request', () => {
		const handler = source.slice(source.indexOf('function handleUnauthorized()'), source.indexOf('// Liquid-glass crossfade'));
		expect(handler).toContain('void bootstrapAuthentication()');
		expect(handler).not.toContain('clearRemoteSession()');
	});
});
