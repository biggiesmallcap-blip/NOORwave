// Curated map of TIDAL mood slugs -> Spotify editorial playlists.
// IDs are stable; contents change daily/weekly server-side. If a fetch
// 404s the card falls back to the glyph -- ship-and-let-degrade is fine.
//
// Only covers moods where Spotify has strong editorial coverage. The
// other TIDAL mood slugs (music_school, credits, social_justice) fall
// through to TIDAL-only content.

export interface SpotifyMoodPlaylist {
	id: string;
	title: string; // fallback display title; Spotify fetch refines this
}

export interface SpotifyMoodCategory {
	slug: string; // matches TIDAL mood slug
	label: string; // display name for landing rail heading
	playlists: SpotifyMoodPlaylist[];
}

export const SPOTIFY_MOOD_CATEGORIES: SpotifyMoodCategory[] = [
	{
		slug: 'mood_party',
		label: 'Party',
		playlists: [
			{ id: '37i9dQZF1DXaXB8fQg7xif', title: 'Dance Party' },
			{ id: '37i9dQZF1DXa2PvUpywmrr', title: 'Party Hits' },
			{ id: '37i9dQZF1DX0BcQWzuB7ZO', title: 'Dance Hits' },
			{ id: '37i9dQZF1DX4dyzvuaRJ0n', title: 'mint' },
		],
	},
	{
		slug: 'mood_workout',
		label: 'Workout',
		playlists: [
			{ id: '37i9dQZF1DXdxcBWuJkbcy', title: 'Beast Mode' },
			{ id: '37i9dQZF1DX35oM5SPECmN', title: 'Power Workout' },
			{ id: '37i9dQZF1DX76Wlfdnj7AP', title: 'Beast Mode Hip-Hop' },
			{ id: '37i9dQZF1DX4eRPd9frC1m', title: 'Adrenaline Workout' },
		],
	},
	{
		slug: 'mood_focus',
		label: 'Focus',
		playlists: [
			{ id: '37i9dQZF1DWZeKCadgRdKQ', title: 'Deep Focus' },
			{ id: '37i9dQZF1DWWQRwui0ExPn', title: 'Lo-Fi Beats' },
			{ id: '37i9dQZF1DX3PFzdbtx1Us', title: 'Beats to Think To' },
			{ id: '37i9dQZF1DWVqfgj8NZEp1', title: 'Jazz Vibes' },
		],
	},
	{
		slug: 'mood_relax',
		label: 'Relax',
		playlists: [
			{ id: '37i9dQZF1DX3Ogo9pFvBkY', title: 'Ambient Chill' },
			{ id: '37i9dQZF1DWVV27DiNWxkR', title: 'Chill Hits' },
			{ id: '37i9dQZF1DX0MLFaUdXnjA', title: 'Acoustic Chill' },
			{ id: '37i9dQZF1DXcCnTAt8CfNe', title: 'Deep Sleep' },
		],
	},
	{
		slug: 'mood_sleep',
		label: 'Sleep',
		playlists: [
			{ id: '37i9dQZF1DWZd79rJ6a7lp', title: 'Sleep' },
			{ id: '37i9dQZF1DXcCnTAt8CfNe', title: 'Deep Sleep' },
			{ id: '37i9dQZF1DWUKPeBypcpcP', title: 'Bedtime Beats' },
			{ id: '37i9dQZF1DWYcDQ1hSjOpY', title: 'Sleepy Piano' },
		],
	},
	{
		slug: 'mood_love',
		label: 'Love',
		playlists: [
			{ id: '37i9dQZF1DX50QitC6Oqtn', title: 'Love Pop' },
			{ id: '37i9dQZF1DX7rOY2tZUw1k', title: 'Romance Latino' },
			{ id: '37i9dQZF1DWXbttAJcbphz', title: 'Pop Romance' },
			{ id: '37i9dQZF1DXbITWG1ZJKYt', title: 'Jazz in the Background' },
		],
	},
	{
		slug: 'm_happy',
		label: 'Happy',
		playlists: [
			{ id: '37i9dQZF1DXdPec7aLTmlC', title: 'Happy Hits!' },
			{ id: '37i9dQZF1DX3rxVfibe1L0', title: 'Mood Booster' },
			{ id: '37i9dQZF1DX9XIFQuFvzM4', title: 'Happy Beats' },
			{ id: '37i9dQZF1DX0vHZ8elq0UK', title: 'Have a Great Day!' },
		],
	},
	{
		slug: 'm_celebration',
		label: 'Celebration',
		playlists: [
			{ id: '37i9dQZF1DXaXB8fQg7xif', title: 'Dance Party' },
			{ id: '37i9dQZF1DXa2PvUpywmrr', title: 'Party Hits' },
			{ id: '37i9dQZF1DX0BcQWzuB7ZO', title: 'Dance Hits' },
			{ id: '37i9dQZF1DXdPec7aLTmlC', title: 'Happy Hits!' },
		],
	},
	{
		slug: 'mood_djselector',
		label: 'DJ Selector',
		playlists: [
			{ id: '37i9dQZF1DX0BcQWzuB7ZO', title: 'Dance Hits' },
			{ id: '37i9dQZF1DX8tZsk68tuDw', title: 'Dance Rising' },
			{ id: '37i9dQZF1DXa8NOEUWPn9W', title: 'Housewerk' },
			{ id: '37i9dQZF1DX4dyzvuaRJ0n', title: 'mint' },
		],
	},
];

export const SPOTIFY_MOODS_BY_SLUG = new Map<string, SpotifyMoodCategory>(
	SPOTIFY_MOOD_CATEGORIES.map((c) => [c.slug, c]),
);
