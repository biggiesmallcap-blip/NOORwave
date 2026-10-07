<script lang="ts">
	import type { TidalSearchVideo, VideoStationCard } from '$lib/api/client';
	import GuideRow from './GuideRow.svelte';
	import { stationFrames } from './stations';

	// One channel in the station guide. A frame starts the station from that
	// video.
	let { card, number, meta, busy = false, onAir = false, rise = null, onplay }: {
		card: VideoStationCard;
		number: string;
		meta: string;
		busy?: boolean;
		onAir?: boolean;
		rise?: number | null;
		onplay: (card: VideoStationCard, startWith?: TidalSearchVideo) => void;
	} = $props();

	const compact = new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 });
</script>

<GuideRow
	title={card.title}
	titleHint={card.subtitle}
	{meta}
	count={card.unwatched_count > 0 ? `${compact.format(card.unwatched_count)} new` : null}
	{number}
	label={`${card.title} station`}
	{onAir}
	{busy}
	frames={stationFrames(card)}
	{rise}
	onplay={(startWith) => onplay(card, startWith)}
/>
