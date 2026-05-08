#!/usr/bin/env node
// Catches `style="font-..."` attributes in Svelte templates.
// stylelint scans <style> blocks via postcss-html but cannot see template
// inline style attributes — this script fills that gap.

import { readFileSync, readdirSync } from 'node:fs';
import { join, extname } from 'node:path';

const ROOT = 'src';
const PATTERN = /style="[^"]*font-/;

function walk(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...walk(path));
    } else if (extname(path) === '.svelte') {
      out.push(path);
    }
  }
  return out;
}

const offenders = [];
for (const file of walk(ROOT)) {
  const lines = readFileSync(file, 'utf8').split('\n');
  lines.forEach((line, idx) => {
    if (PATTERN.test(line)) {
      offenders.push(`${file}:${idx + 1}: ${line.trim()}`);
    }
  });
}

if (offenders.length > 0) {
  console.error(`\nInline font styles found in ${offenders.length} location(s):\n`);
  for (const o of offenders) console.error('  ' + o);
  console.error('\nMove these to a scoped <style> block using token-based font-size / font-family / font-weight / line-height.');
  console.error('See frontend/STYLING.md for the typography token scale.\n');
  process.exit(1);
}

console.log('No inline font styles in templates.');
