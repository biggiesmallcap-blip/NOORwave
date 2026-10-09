import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

describe('1.0 welcome', () => {
	const layout = readFileSync('src/routes/+layout.svelte', 'utf8');
	const welcome = readFileSync('src/lib/components/onboarding/WelcomeRelease.svelte', 'utf8');

	test('stays off onboarding, the phone remote and the connect screen', () => {
		const mount = layout.slice(layout.indexOf('<WelcomeRelease'), layout.indexOf('/>', layout.indexOf('<WelcomeRelease')));
		expect(mount).toContain('onboardingChecked');
		expect(mount).toContain('!isOnboardingRoute');
		expect(mount).toContain('!isRemoteRoute');
		expect(mount).toContain("startsWith('/connect')");
		expect(mount).toContain('version={serverVersion}');
	});

	test('shows once, never on top of another dialog, and survives a full store', () => {
		expect(welcome).toContain("SEEN_KEY = 'noor.welcome.v1-0.seen'");
		expect(welcome).toMatch(/try \{\s*return localStorage\.getItem/);
		expect(welcome).toMatch(/try \{\s*localStorage\.setItem/);
		expect(welcome).toContain('.patch-info-backdrop');
	});
});
