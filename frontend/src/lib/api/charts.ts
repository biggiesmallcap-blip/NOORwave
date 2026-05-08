import { api } from './client';
export type { ChartEntry, LastfmCountry, LastfmGenre, Track } from './client';

export const getTrending = api.getTrending.bind(api);
export const getLastfmCountries = api.getLastfmCountries.bind(api);
export const getLastfmGenres = api.getLastfmGenres.bind(api);
