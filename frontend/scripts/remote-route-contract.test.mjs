import { describe, expect, test } from 'vitest';
import { readFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(__dirname, '..');
const routePath = resolve(root, 'src/routes/remote/+page.svelte');
const layoutPath = resolve(root, 'src/routes/+layout.svelte');

function read(path) {
	return readFileSync(path, 'utf8');
}

describe('remote route contract', () => {
	test('has a dedicated route file', () => {
		expect(existsSync(routePath)).toBe(true);
	});

	test('root layout exposes stripped chrome for remote', () => {
		const layout = read(layoutPath);
		expect(layout).toContain("page.url.pathname.startsWith('/remote')");
		expect(layout).toContain('remote-shell');
	});

	test('remote page does not use inline style attributes', () => {
		const page = read(routePath);
		expect(page).not.toMatch(/\sstyle=/);
	});

	test('remote imports the dedicated transport component', () => {
		const page = read(routePath);
		expect(page).toContain("RemoteTransport from '$lib/components/remote/RemoteTransport.svelte'");
		expect(page).toContain('<RemoteTransport');
	});

	test('transport exposes expected playback controls', () => {
		const transportPath = resolve(root, 'src/lib/components/remote/RemoteTransport.svelte');
		const transport = read(transportPath);
		for (const label of ['Previous', 'Play or pause', 'Next', 'Seek playback', 'Volume']) {
			expect(transport).toContain(`aria-label="${label}"`);
		}
	});
});
