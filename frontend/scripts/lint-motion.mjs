#!/usr/bin/env node
// Motion lint (audit "Motion", STYLING.md "Motion"): transitions and
// animations should use the motion tokens (--motion-fast, --motion-base,
// --motion-slow, --motion-exit, --motion-press), which bundle their easing.
// Raw durations ("0.15s", "120ms ease") still work, so this warns instead of
// failing; it reports a count by default and every location with --verbose.

import { readFileSync, readdirSync } from 'node:fs';
import { join, extname } from 'node:path';

const ROOT = 'src';
// Shorthands only: the tokens bundle duration and easing, so they cannot be
// used in `*-duration` overrides, which stay raw on purpose.
const PROPERTY = /^\s*(transition|animation)\s*:/;
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
	// The phone remote has its own touch language (spring curves), outside the
	// desktop motion system.
	if (/[\\/]remote[\\/]/.test(file)) continue;
	const lines = readFileSync(file, 'utf8').split('\n');
	let continuing = false;
	lines.forEach((line, index) => {
		const starts = PROPERTY.test(line);
		if (!starts && !continuing) return;
		// Multi-line transition lists continue until the declaration ends.
		continuing = !line.includes(';');
		// Reduced-motion overrides set 1ms on purpose, and looping animations
		// (spinners, shimmer, pulses) keep their own period.
		if (/\b1ms\b/.test(line) || /\b0s\b/.test(line) || /\binfinite\b/.test(line)) return;
		// Linear timing tracks real time (progress fills); `motion-ok` marks a
		// deliberate exception with its reason on the same line.
		if (/\blinear\b/.test(line) || line.includes('motion-ok')) return;
		// A raw value after a token is a delay, not a duration.
		if (line.includes('var(--motion-')) return;
		if (RAW.test(line)) hits.push(`${file}:${index + 1}: ${line.trim()}`);
	});
}

if (hits.length > 0) {
	console.warn(`Motion: ${hits.length} transition/animation declaration(s) use raw durations instead of --motion-* tokens.`);
	if (verbose) for (const hit of hits) console.warn('  ' + hit);
	else console.warn('Run `node scripts/lint-motion.mjs --verbose` to list them (see frontend/STYLING.md "Motion").');
}
