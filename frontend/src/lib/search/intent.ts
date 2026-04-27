import { parseQuery } from './query_parser';

export type IntentKind =
  | { type: 'play' }
  | { type: 'radio' }
  | { type: 'year_filter'; year: number }
  | { type: 'none' };

export interface ParsedIntent {
  free_text: string;
  intent: IntentKind;
  extra_filters: Record<string, import('./query_parser').FilterValue>;
}

const PLAY_RE = /^play\s+(.+)$/i;
const RADIO_RE = /^(?:similar\s+to|like)\s+(.+)$/i;
// 4-digit year at the end of the query after a space
const YEAR_SUFFIX_RE = /^(.*?)\s+((?:19|20)\d{2})$/;

export function parseIntent(raw: string): ParsedIntent {
  const trimmed = raw.trim();

  // "/play <query>" and "/queue <query>" are handled by slash commands — skip
  if (trimmed.startsWith('/')) {
    return { free_text: trimmed, intent: { type: 'none' }, extra_filters: {} };
  }

  // "play <query>" → play mode
  const playMatch = trimmed.match(PLAY_RE);
  if (playMatch) {
    const inner = parseQuery(playMatch[1]);
    return { free_text: inner.free_text, intent: { type: 'play' }, extra_filters: inner.filters };
  }

  // "similar to <query>" / "like <query>" → radio
  const radioMatch = trimmed.match(RADIO_RE);
  if (radioMatch) {
    const inner = parseQuery(radioMatch[1]);
    return { free_text: inner.free_text, intent: { type: 'radio' }, extra_filters: inner.filters };
  }

  // "<artist/title> <4-digit-year>" → auto year filter
  const yearMatch = trimmed.match(YEAR_SUFFIX_RE);
  if (yearMatch) {
    const year = parseInt(yearMatch[2], 10);
    const inner = parseQuery(yearMatch[1]);
    // Only apply if no explicit year filter already in the text
    if (!inner.filters['year']) {
      return {
        free_text: inner.free_text,
        intent: { type: 'year_filter', year },
        extra_filters: { ...inner.filters, year: { type: 'exact', value: String(year) } },
      };
    }
  }

  // No intent recognised — parse normally
  const parsed = parseQuery(trimmed);
  return { free_text: parsed.free_text, intent: { type: 'none' }, extra_filters: parsed.filters };
}
