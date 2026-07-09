import { describe, expect, test } from 'vitest';
import { stripFilter } from './query_parser';

describe('stripFilter', () => {
  test('removes an exact key:value token, keeping free text and other filters', () => {
    expect(stripFilter('burial bpm:128 key:Am', 'bpm')).toBe('burial key:Am');
  });

  test('removes comparison forms', () => {
    expect(stripFilter('energy:>0.7 chill', 'energy')).toBe('chill');
    expect(stripFilter('year>=2010 year<=2019', 'year')).toBe('');
  });

  test('leaves the query untouched when the key is absent', () => {
    expect(stripFilter('deep house vibes', 'bpm')).toBe('deep house vibes');
  });

  test('collapses surrounding whitespace', () => {
    expect(stripFilter('  bpm:128   genre:dnb  ', 'bpm')).toBe('genre:dnb');
  });

  test('only strips the targeted key, not keys that share a prefix', () => {
    // 'key' must not also strip 'keyx:' style tokens with a different key name
    expect(stripFilter('key:Am camelot:8A', 'key')).toBe('camelot:8A');
  });
});
