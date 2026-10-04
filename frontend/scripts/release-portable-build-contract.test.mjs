import { describe, expect, test } from 'vitest';
import { chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { delimiter, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { execFileSync, spawnSync } from 'node:child_process';

const root = resolve(import.meta.dirname, '../..');
const powershell = process.platform === 'win32' ? 'powershell.exe' : 'pwsh';
const skipPackaging = process.platform !== 'win32' && !process.env.CI &&
	spawnSync(powershell, ['-NoProfile', '-Command', 'exit 0']).status !== 0;

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
		expect(script).toContain('Invoke-Native -FilePath pnpm -Arguments @("run", "build")');
		expect(script).toContain('frontend\\build');
		expect(script).toContain('index.html');
	});

	test('Windows compiles once, archives the portable binary, then bundles the signed installer', () => {
		const workflow = read('.github/workflows/release.yml').split('  build-linux:')[0];
		const compile = 'tauri build --no-bundle --config tauri.installer.conf.json -- --locked';
		const portable = '.\\scripts\\build-portable.ps1 -UsePrebuiltFrontend -UsePrebuiltBinaries';
		const bundle = 'tauri bundle --bundles nsis --config tauri.installer.conf.json';

		expect(workflow).toContain(compile);
		expect(read('.github/workflows/warm-cache.yml')).toContain(compile);
		expect(workflow.indexOf(compile)).toBeLessThan(workflow.indexOf(portable));
		expect(workflow.indexOf(portable)).toBeLessThan(workflow.indexOf(bundle));
		expect(workflow).not.toContain('tauri build --bundles');
		expect(workflow).toContain('TAURI_SIGNING_PRIVATE_KEY:');
	});
});

// Ubuntu PR runners have pwsh, so packaging behavior is checked in normal CI
// as well as locally on Windows. No Rust compilation or real executable runs.
// Other local hosts can skip if PowerShell is absent; CI must run these tests.
describe.skipIf(skipPackaging)('prebuilt Windows portable packaging', () => {
	function withFixture(run) {
		const prefix = join(tmpdir(), 'noor-portable-');
		const fixture = mkdtempSync(prefix);
		try {
			for (const directory of ['scripts', 'frontend/build', 'target/release', 'bin']) {
				mkdirSync(join(fixture, directory), { recursive: true });
			}
			cpSync(resolve(root, 'scripts/build-portable.ps1'), join(fixture, 'scripts/build-portable.ps1'));
			writeFileSync(join(fixture, 'frontend/build/index.html'), '<html>fixture UI</html>');
			writeFileSync(join(fixture, 'target/release/noor-app.exe'), 'fixture app');
			writeFileSync(join(fixture, 'target/release/noor-server.exe'), 'fixture server');
			// Prebuilt packaging must not invoke either build tool.
			for (const tool of ['cargo', 'pnpm']) {
				const windows = process.platform === 'win32';
				const mock = join(fixture, 'bin', windows ? `${tool}.cmd` : tool);
				writeFileSync(mock, windows ? '@echo off\r\nexit /b 91\r\n' : '#!/bin/sh\nexit 91\n');
				if (!windows) chmodSync(mock, 0o755);
			}
			const env = { ...process.env, PATH: join(fixture, 'bin') + delimiter + process.env.PATH };
			const packagePortable = () => execFileSync(powershell, [
				'-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', join(fixture, 'scripts/build-portable.ps1'),
				'-UsePrebuiltFrontend', '-UsePrebuiltBinaries'
			], { env, encoding: 'utf8', timeout: 20000, stdio: 'pipe' });
			run(fixture, packagePortable);
		} finally {
			if (!fixture.startsWith(prefix)) throw new Error('Fixture path escaped the test directory');
			rmSync(fixture, { recursive: true, force: true });
		}
	}

	test('archives the existing executables and UI without invoking a compiler', () => {
		withFixture((fixture, packagePortable) => {
			expect(packagePortable()).toContain('Build complete!');
			expect(existsSync(join(fixture, 'dist/NOORwave-portable.zip'))).toBe(true);
			execFileSync(powershell, [
				'-NoProfile', '-Command',
				"Expand-Archive -LiteralPath (Join-Path $env:NOOR_PORTABLE_FIXTURE 'dist/NOORwave-portable.zip') -DestinationPath (Join-Path $env:NOOR_PORTABLE_FIXTURE 'unpacked')"
			], { env: { ...process.env, NOOR_PORTABLE_FIXTURE: fixture }, stdio: 'pipe', timeout: 20000 });
			for (const [path, content] of [
				['NOORwave.exe', 'fixture app'],
				['noor-server.exe', 'fixture server'],
				['www/index.html', '<html>fixture UI</html>']
			]) {
				expect(readFileSync(join(fixture, 'unpacked/NOORwave', path), 'utf8')).toBe(content);
			}
		});
	}, 30000);

	for (const binary of ['noor-app.exe', 'noor-server.exe']) {
		test(`fails before packaging when ${binary} is missing`, () => {
			withFixture((fixture, packagePortable) => {
				rmSync(join(fixture, 'target/release', binary));
				let failure;
				try {
					packagePortable();
				} catch (error) {
					failure = error;
				}
				expect(failure).toBeDefined();
				expect(String(failure.stderr)).toContain(`${binary} does not exist`);
				expect(existsSync(join(fixture, 'dist'))).toBe(false);
			});
		}, 30000);
	}
});
