import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../..');

function read(relativePath) {
	return readFileSync(resolve(root, relativePath), 'utf8');
}

describe('Windows release portable build', () => {
	test('GitHub builds the frontend before packaging and skips the script frontend build', () => {
		const workflow = read('.github/workflows/release.yml');

		expect(workflow).toContain('name: Build frontend');
		expect(workflow).toContain('pnpm run build');
		expect(workflow).toContain('.\\scripts\\build-portable.ps1 -UsePrebuiltFrontend');
	});

	test('portable build script can build frontend locally or validate a prebuilt UI', () => {
		const script = read('scripts/build-portable.ps1');

		expect(script).toContain('[switch]$UsePrebuiltFrontend');
		expect(script).toContain('Invoke-Native pnpm run build');
		expect(script).toContain('frontend\\build');
		expect(script).toContain('index.html');
	});
});
