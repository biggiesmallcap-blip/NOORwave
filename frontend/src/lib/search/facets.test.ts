import { describe, expect, test } from 'vitest';
import { FACETS, matchFacets, inlineCompletionFor, facetForToken } from './facets';
import { SUPPORTED_KEYS } from './query_parser';

describe('facet descriptors', () => {
  test('every suggested facet maps to a supported filter key', () => {
    // We must never suggest a filter the parser would silently drop.
    for (const facet of FACETS) {
      expect(SUPPORTED_KEYS.has(facet.key)).toBe(true);
      expect(facet.token).toBe(`${facet.key}:`);
    }
  });

  test('covers every supported key except the redundant vocal alias', () => {
    const suggested = new Set(FACETS.map((f) => f.key));
    for (const key of SUPPORTED_KEYS) {
      if (key === 'vocal') continue; // redundant with instrumental, intentionally hidden
      expect(suggested.has(key)).toBe(true);
    }
    expect(suggested.has('vocal')).toBe(false);
  });
});

describe('matchFacets', () => {
  test('empty tail returns the full list', () => {
    expect(matchFacets('')).toEqual(FACETS);
  });

  test('narrows by key prefix', () => {
    expect(matchFacets('be').map((f) => f.key)).toEqual([]); // no key starts with 'be'
    expect(matchFacets('bp').map((f) => f.key)).toEqual(['bpm']);
    expect(matchFacets('a').map((f) => f.key)).toEqual(['artist', 'album']);
  });

  test('narrows by label prefix too', () => {
    expect(matchFacets('tempo').map((f) => f.key)).toEqual(['bpm']);
  });

  test('a chosen key (contains colon) suggests nothing', () => {
    expect(matchFacets('key:')).toEqual([]);
  });
});

describe('inlineCompletionFor', () => {
  test('completes unique prefixes of two or more chars', () => {
    expect(inlineCompletionFor('bp')).toBe('bpm:');
    expect(inlineCompletionFor('ke')).toBe('key:');
    expect(inlineCompletionFor('ca')).toBe('camelot:');
    expect(inlineCompletionFor('en')).toBe('energy:');
    expect(inlineCompletionFor('ge')).toBe('genre:');
    expect(inlineCompletionFor('al')).toBe('album:');
    expect(inlineCompletionFor('ar')).toBe('artist:');
    expect(inlineCompletionFor('in')).toBe('instrumental:');
  });

  test('completes a full key spelled out', () => {
    expect(inlineCompletionFor('genre')).toBe('genre:');
  });

  test('is case-insensitive and trims', () => {
    expect(inlineCompletionFor('  BPM ')).toBe('bpm:');
  });

  test('refuses single-letter and ambiguous or absent prefixes', () => {
    expect(inlineCompletionFor('a')).toBeNull(); // too short + ambiguous
    expect(inlineCompletionFor('zz')).toBeNull();
    expect(inlineCompletionFor('')).toBeNull();
  });

  test('does not complete an already-typed key:value token', () => {
    expect(inlineCompletionFor('bpm:128')).toBeNull();
  });
});

describe('facetForToken', () => {
  test('resolves to the descriptor for a unique prefix', () => {
    expect(facetForToken('ke')?.key).toBe('key');
    expect(facetForToken('zz')).toBeNull();
  });
});
