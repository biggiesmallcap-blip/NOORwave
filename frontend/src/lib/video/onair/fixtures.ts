// Test fixtures for On Air: a small lineup and a guide in the exact wire
// shapes of video_explore_types.ts. Only tests import this.
import type {
	ChannelGuideResponse,
	ChannelProgram,
	ChannelTuneResponse,
	ExploreVideo,
	VideoChannel,
	VideoChannelsResponse,
} from '$lib/api/video_explore_types';

export function fixtureVideo(id: number, artistId: number, artist: string, title: string, durationS = 240): ExploreVideo {
	return {
		tidal_id: id,
		title,
		duration_ms: durationS * 1000,
		artist_id: artistId,
		artist_name: artist,
		album_tidal_id: null,
		artwork_url: `https://resources.tidal.com/images/${id}/320x180.jpg`,
		quality: null,
		explicit: null,
		type: 'Music Video',
	};
}

export function fixtureProgram(slot: number, startS: number, video: ExploreVideo, why = ''): ChannelProgram {
	return { slot, start_s: startS, duration_s: (video.duration_ms ?? 0) / 1000, video, why };
}

/** 21:00:00 local, for every fixture clock. */
export const FIXTURE_NOW_S = 21 * 3600;

function channel(partial: Partial<VideoChannel> & Pick<VideoChannel, 'id' | 'no' | 'title'>): VideoChannel {
	return {
		subtitle: '',
		hue: null,
		scene_id: null,
		locked: false,
		unlock: null,
		now: null,
		next: null,
		then: null,
		...partial,
	};
}

const coldplay = fixtureVideo(101, 11, 'Coldplay', 'Viva la Vida (Official Video)', 242);
const arcade = fixtureVideo(102, 12, 'Arcade Fire', 'Wake Up (Live at Coachella)', 300);
const macdre = fixtureVideo(201, 21, 'Mac Dre', 'Feelin Myself', 210);
const e40 = fixtureVideo(202, 22, 'E-40', 'Tell Me When to Go', 230);
const westlife = fixtureVideo(301, 31, 'Westlife', 'Swear It Again (Live at Croke Park)', 260);
const bts = fixtureVideo(401, 41, 'BTS', 'Dynamite', 200);

/** Channels in appetite order (the server's order), not number order. */
export const fixtureChannels: VideoChannelsResponse = {
	day: '2026-10-14',
	now_s: FIXTURE_NOW_S,
	explore: 'mixed',
	building: false,
	channels: [
		channel({
			id: 'scene:7', no: 12, title: 'Bay Area Rap', subtitle: 'Mac Dre, E-40 and the Yay',
			hue: 32, scene_id: 7,
			now: { ...fixtureProgram(40, FIXTURE_NOW_S - 60, macdre, 'Near E-40, 12 plays'), offset_s: 60 },
			next: fixtureProgram(41, FIXTURE_NOW_S + 150, e40),
		}),
		channel({
			id: 'your', no: 1, title: 'Your Channel', subtitle: 'Your library, on camera',
			now: { ...fixtureProgram(10, FIXTURE_NOW_S - 200, coldplay, 'In your library'), offset_s: 200 },
			next: fixtureProgram(11, FIXTURE_NOW_S + 42, arcade),
		}),
		channel({
			id: 'scene:3', no: 14, title: 'K-pop', subtitle: 'BTS & BLACKPINK', hue: 300, scene_id: 3,
			now: { ...fixtureProgram(5, FIXTURE_NOW_S - 20, bts), offset_s: 20 },
			next: null,
		}),
		channel({
			id: 'concerts', no: 6, title: 'Concert Hall', subtitle: 'Whole concerts, back to back',
			now: { ...fixtureProgram(2, FIXTURE_NOW_S - 100, westlife, 'Live at Croke Park, 2008'), offset_s: 100 },
			next: null,
		}),
		channel({
			id: 'rewind', no: 7, title: 'Rewind', subtitle: 'The decade of the day', locked: true,
			unlock: { hours: 12, need_hours: 30 },
		}),
		channel({
			id: 'scene:9', no: 21, title: 'Drum and Bass', subtitle: 'Coming soon', hue: 160, scene_id: 9, locked: true,
			unlock: { hours: 18.5, need_hours: 30 },
		}),
	],
};

export const fixtureGuide: ChannelGuideResponse = {
	channel_id: 'scene:7',
	day: '2026-10-14',
	programs: [
		fixtureProgram(39, FIXTURE_NOW_S - 270, e40),
		fixtureProgram(40, FIXTURE_NOW_S - 60, macdre),
		fixtureProgram(41, FIXTURE_NOW_S + 150, e40),
		fixtureProgram(42, FIXTURE_NOW_S + 380, macdre),
	],
};

/** A tune answer for Bay Area Rap at `offsetS` into Mac Dre. */
export function fixtureTune(offsetS: number): ChannelTuneResponse {
	const bay = fixtureChannels.channels[0];
	return {
		channel: { ...bay, now: bay.now ? { ...bay.now, offset_s: offsetS } : null },
		video: macdre,
		offset_s: offsetS,
		queue: [e40, fixtureVideo(203, 23, 'Too Short', 'Blow the Whistle', 220), fixtureVideo(204, 21, 'Mac Dre', 'Thizzle Dance', 200)],
	};
}
