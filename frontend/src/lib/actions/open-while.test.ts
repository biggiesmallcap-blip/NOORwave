import { describe, expect, it } from 'vitest';
import { readdirSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { openWhile } from './open-while';

function details(open = false) {
	return { open } as HTMLDetailsElement;
}

describe('openWhile', () => {
	it('opens the panel when the job is already running', () => {
		const node = details();
		openWhile(node, true);
		expect(node.open).toBe(true);
	});

	it('leaves a panel the user opened alone while idle', () => {
		const node = details();
		const action = openWhile(node, false);
		node.open = true;
		action.update(false);
		expect(node.open).toBe(true);
	});

	it('opens when a job starts and does not close when it ends', () => {
		const node = details();
		const action = openWhile(node, false);
		action.update(true);
		expect(node.open).toBe(true);
		action.update(false);
		expect(node.open).toBe(true);
	});

	it('lets the user close the panel while the job keeps running', () => {
		const node = details();
		const action = openWhile(node, true);
		node.open = false;
		action.update(true);
		expect(node.open).toBe(false);
	});
});

describe('no <details open={...}> bindings', () => {
	it('panels never re-assign open on every reactive update', () => {
		const root = resolve(import.meta.dirname, '../..');
		const offenders = readdirSync(root, { recursive: true, encoding: 'utf8' })
			.filter((file) => file.endsWith('.svelte'))
			.filter((file) => /<details\b[^>]*\sopen=\{/.test(readFileSync(resolve(root, file), 'utf8')));
		expect(offenders).toEqual([]);
	});
});
