// Facet descriptors: the single source of truth for the search fill suggestions
// (the focus-when-empty facet-name popover and the inline Tab-completion). Each
// entry pairs a supported filter key with a human label + example so the
// suggestion UI is self-documenting. Value suggestions (musical keys, Camelot
// wheel, genre lists) are intentionally NOT modelled here yet - they are a
// deferred follow-up; this file is shaped so they can be added without touching
// callers.
import { SUPPORTED_KEYS } from '$lib/search/query_parser';

export interface FacetDescriptor {
  key: string; // 'bpm'
  token: string; // 'bpm:'
  label: string; // 'Tempo'
  description: string; // 'Beats per minute'
  example: string; // 'bpm:128'
}

// Ordered for the popover: audio/DSP facets first, then text, then boolean.
// `vocal` is a supported parse key but intentionally omitted from suggestions
// because it is redundant with `instrumental` (vocal:false === instrumental:true).
const FACET_META: { key: string; label: string; description: string; example: string }[] = [
  { key: 'bpm', label: 'Tempo', description: 'Beats per minute, single or range', example: 'bpm:128' },
  { key: 'key', label: 'Musical key', description: 'Key signature, e.g. Am or F#', example: 'key:Am' },
  { key: 'camelot', label: 'Camelot key', description: 'Harmonic mixing wheel, e.g. 8A', example: 'camelot:8A' },
  { key: 'energy', label: 'Energy', description: 'Intensity from 0 to 1', example: 'energy:>0.7' },
  { key: 'danceability', label: 'Danceability', description: 'Groove from 0 to 1', example: 'danceability:>0.6' },
  { key: 'year', label: 'Year', description: 'Release year or range', example: 'year:2015-2019' },
  { key: 'genre', label: 'Genre', description: 'Style or subgenre', example: 'genre:dnb' },
  { key: 'artist', label: 'Artist', description: 'Artist name contains', example: 'artist:burial' },
  { key: 'album', label: 'Album', description: 'Album title contains', example: 'album:untrue' },
  { key: 'instrumental', label: 'Instrumental', description: 'Instrumental tracks only', example: 'instrumental:true' },
];

export const FACETS: FacetDescriptor[] = FACET_META.filter((m) => SUPPORTED_KEYS.has(m.key)).map(
  (m) => ({ key: m.key, token: `${m.key}:`, label: m.label, description: m.description, example: m.example })
);

// Narrow the facet list as the user types a bare trailing word. Empty tail
// returns the full list (focus-when-empty). A tail containing ':' means a key
// has already been chosen, so there is nothing left to suggest.
export function matchFacets(rawTail: string): FacetDescriptor[] {
  const tail = rawTail.trim().toLowerCase();
  if (!tail) return FACETS;
  if (tail.includes(':')) return [];
  return FACETS.filter((f) => f.key.startsWith(tail) || f.label.toLowerCase().startsWith(tail));
}

// Inline completion: when the trailing word uniquely prefixes one facet key
// (min two chars to avoid single-letter noise), return the token to complete to.
// Replaces the search page's hardcoded FILTER_PREFIXES map with a prefix match
// over the full facet set. Returns null when ambiguous, too short, or already a
// key:value token.
export function inlineCompletionFor(rawTail: string): string | null {
  const tail = rawTail.trim().toLowerCase();
  if (tail.length < 2) return null;
  if (tail.includes(':')) return null;
  const matches = FACETS.filter((f) => f.key.startsWith(tail));
  if (matches.length !== 1) return null;
  return matches[0].token;
}

// The single facet a trailing word resolves to, or null. Thin wrapper over
// inlineCompletionFor for callers that want the descriptor, not just the token.
export function facetForToken(rawTail: string): FacetDescriptor | null {
  const token = inlineCompletionFor(rawTail);
  if (!token) return null;
  return FACETS.find((f) => f.token === token) ?? null;
}
