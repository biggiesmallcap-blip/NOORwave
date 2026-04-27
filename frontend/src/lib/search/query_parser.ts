export type FilterValue =
  | { type: 'exact'; value: string }
  | { type: 'range'; min: number; max: number }
  | { type: 'comparison'; op: '>' | '<' | '>=' | '<='; value: number }
  | { type: 'multi'; values: string[] };

export interface ParsedQuery {
  free_text: string; // everything that is NOT a filter token, trimmed
  filters: Record<string, FilterValue>;
}

const SUPPORTED_KEYS = new Set([
  'bpm',
  'key',
  'camelot',
  'energy',
  'danceability',
  'year',
  'genre',
  'artist',
  'album',
  'type',
  'quality',
  'mood',
  'vocal'
]);

export function parseQuery(input: string): ParsedQuery {
  const tokens = input.trim().split(/\s+/).filter(t => t.length > 0);
  const filters: Record<string, FilterValue> = {};
  const freeTextTokens: string[] = [];

  for (const token of tokens) {
    const filterResult = parseToken(token);
    if (filterResult) {
      const { key, value } = filterResult;
      filters[key] = value;
    } else {
      freeTextTokens.push(token);
    }
  }

  return {
    free_text: freeTextTokens.join(' ').trim(),
    filters
  };
}

function parseToken(
  token: string
): { key: string; value: FilterValue } | null {
  // Try comparison operators first (key>value, key<value, key>=value, key<=value)
  const comparisonMatch = token.match(/^([a-z_]+)(>=|<=|>|<)(.+)$/);
  if (comparisonMatch) {
    const [, keyPart, op, valuePart] = comparisonMatch;
    if (SUPPORTED_KEYS.has(keyPart)) {
      const num = parseFloat(valuePart);
      if (!isNaN(num)) {
        return {
          key: keyPart,
          value: {
            type: 'comparison',
            op: op as '>' | '<' | '>=' | '<=',
            value: num
          }
        };
      }
    }
  }

  // Try exact/range/multi filters (key:value format)
  const colonMatch = token.match(/^([a-z_]+):(.+)$/);
  if (colonMatch) {
    const [, keyPart, valuePart] = colonMatch;
    if (!SUPPORTED_KEYS.has(keyPart)) {
      return null; // unknown key
    }

    // Check for multi-value (a|b|c)
    if (valuePart.includes('|')) {
      const values = valuePart.split('|').map(v => v.trim()).filter(v => v.length > 0);
      if (values.length > 0) {
        return {
          key: keyPart,
          value: { type: 'multi', values }
        };
      }
    }

    // Check for range (min-max, both numeric)
    const rangeParts = valuePart.split('-');
    if (rangeParts.length === 2) {
      const min = parseFloat(rangeParts[0]);
      const max = parseFloat(rangeParts[1]);
      if (!isNaN(min) && !isNaN(max)) {
        return {
          key: keyPart,
          value: { type: 'range', min, max }
        };
      }
    }

    // Default to exact match
    return {
      key: keyPart,
      value: { type: 'exact', value: valuePart }
    };
  }

  return null;
}

export function filtersToChips(
  filters: Record<string, FilterValue>
): Array<{ key: string; display: string }> {
  return Object.entries(filters).map(([key, value]) => {
    let display: string;

    switch (value.type) {
      case 'exact':
        display = `${key}:${value.value}`;
        break;
      case 'range':
        display = `${key}:${value.min}-${value.max}`;
        break;
      case 'comparison':
        display = `${key}${value.op}${value.value}`;
        break;
      case 'multi':
        display = `${key}:${value.values.join('|')}`;
        break;
    }

    return { key, display };
  });
}
