import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

const read = (path) => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');

describe('GitHub CI configuration', () => {
	test('PR checks run Rust tests, clippy, frontend type checks, and frontend tests', () => {
		const workflow = read('.github/workflows/pr-check.yml');

		expect(workflow).toContain('cargo test --workspace --locked');
		expect(workflow).toContain('cargo clippy --workspace --all-targets --locked');
		expect(workflow).toContain('continue-on-error: true');
		expect(workflow).toContain('pnpm check');
		expect(workflow).toContain('pnpm test');
	});

	test('Dependabot covers Actions, Cargo, and frontend dependencies with guarded groups', () => {
		const config = read('.github/dependabot.yml');

		expect(config).toContain('package-ecosystem: github-actions');
		expect(config).toContain('package-ecosystem: cargo');
		expect(config).toContain('package-ecosystem: npm');
		expect(config).toContain('dependency-name: cpal');
		expect(config).toContain('dependency-name: tauri');
		expect(config).toContain('dependency-name: "@tauri-apps/*"');
	});

	test('release workflow serializes tag builds and validates checksum artifacts', () => {
		const workflow = read('.github/workflows/release.yml');

		expect(workflow).toContain('group: release-${{ github.ref_name }}');
		expect(workflow).toContain('Test-Path');
		expect(workflow).toContain('Expected 4 checksum files');
	});
});
