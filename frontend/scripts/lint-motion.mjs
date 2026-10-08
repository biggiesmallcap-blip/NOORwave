#!/usr/bin/env node
// Motion lint (audit "Motion", STYLING.md "Motion"): transitions and
// animations should use the motion tokens (--motion-fast, --motion-base,
// --motion-slow, --motion-exit, --motion-press), which bundle their easing.
// Raw durations ("0.15s", "120ms ease") still work, so this warns instead of
// failing; it reports a count by default and every location with --verbose.

import { readFileSync, readdirSync } from 'node:fs';
import { join, extname } from 'node:path';

const ROOT = 'src';
const PROPERTY = /^\s*(transition|transition-duration|animation|animation-duration)\s*:/;
const RAW = /(?<![\w-])\d*\.?\d+m?s\b/;
const verbose = process.argv.includes('--verbose');

function walk(dir) {
	const out = [];
	for (const entry of readdirSync(dir, { withFileTypes: true })) {
		const path = join(dir, entry.name);
		if (entry.isDirectory()) out.push(...walk(path));
		else if (['.svelte', '.css'].includes(extname(path))) out.push(path);
	}
	return out;
}

const hits = [];
for (const file of walk(ROOT)) {
	const lines = readFileSync(file, 'utf8').split('\n');
	let continuing = false;
	lines.forEach((line, index) => {
		const starts = PROPERTY.test(line);
		if (!starts && !continuing) return;
		// Multi-line transition lists continue until the declaration ends.
		continuing = !line.includes(';');
		// Reduced-motion overrides set 1ms on purpose.
		if (/\b1ms\b/.test(line) || /\b0s\b/.test(line)) return;
		if (RAW.test(line.replace(/var\([^)]*\)/g, ''))) hits.push(`${file}:${index + 1}: ${line.trim()}`);
	});
}

if (hits.length > 0) {
	console.warn(`Motion: ${hits.length} transition/animation declaration(s) use raw durations instead of --motion-* tokens.`);
	if (verbose) for (const hit of hits) console.warn('  ' + hit);
	else console.warn('Run `node scripts/lint-motion.mjs --verbose` to list them (see frontend/STYLING.md "Motion").');
}
