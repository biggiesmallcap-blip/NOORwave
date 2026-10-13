<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import type { ExploreMode, VideoChannel } from '$lib/api/video_explore_types';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import ErrorState from '$lib/components/ui/ErrorState.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import GuidePlaceholder from '$lib/components/video/GuidePlaceholder.svelte';
	import { showToast } from '$lib/stores/toast';
	import { videoChannelOnAir } from '$lib/stores/video_session';
	import ChannelCard from '$lib/video/onair/ChannelCard.svelte';
	import ChannelRow from '$lib/video/onair/ChannelRow.svelte';
	import { formatClock, liveSlot, secondsSinceMidnight } from '$lib/video/onair/clock';
	import {
		airTick,
		byNumber,
		channelLineup,
		ensureLineup,
		exploreMode,
		forYouTonight,
		loadExploreMode,
		loadLineup,
		setExploreMode,
	} from '$lib/video/onair/lineup';
	import { tuneChannel } from '$lib/video/onair/tune';
	import { playVideoCollection } from '$lib/video/play_collection';
	import { watchUrl } from '$lib/video/section';

	// On Air: the dial. Every channel runs on a clock, so this page is a live
	// guide of what is on right now: a "For you tonight" strip of the channels
	// you have the most appetite for, then every channel in number order with
	// a bar moving through the current video and what is next. Nothing plays
	// until you pick a channel; picking one tunes in mid-video, where the
	// schedule is (the layout then opens the watch page).

	const EXPLORE_OPTIONS: readonly { value: ExploreMode; label: string }[] = [
		{ value: 'familiar', label: 'Familiar' },
		{ value: 'mixed', label: 'Mixed' },
		{ value: 'adventurous', label: 'Adventurous' },
	];
	// While the server programmes the day (building, no channels yet), look
	// again every few seconds so the dial fills in by itself.
	const BUILD_POLL_MS = 5000;
	const BUILD_POLL_MAX = 24;
	// A channel whose "now" ran out triggers a quiet refetch, at most this often.
	const STALE_REFETCH_MS = 20_000;
	const REFRESH_MS = 5 * 60_000;
	const SCENE_START = 10;

	let error = $state<unknown>(null);
	let tuning = $state<string | null>(null);
	let pollTimer: ReturnType<typeof setTimeout> | null = null;
	let polls = 0;
	let lastFetch = 0;

	let lineup = $derived($channelLineup);
	let channels = $derived(lineup?.data.channels ?? []);
	let elapsed = $derived(lineup ? Math.max(0, ($airTick - lineup.fetchedAt) / 1000) : 0);
	let slots = $derived(new Map(channels.map((channel) => [channel.id, liveSlot(channel, elapsed)])));
	let tonight = $derived(forYouTonight(channels));
	let dial = $derived(byNumber(channels));
	let firstScene = $derived(dial.find((channel) => channel.no >= SCENE_START)?.id ?? null);
	let clock = $derived(formatClock(secondsSinceMidnight(new Date($airTick))));
	let building = $derived(Boolean(lineup?.data.building));
	let onAirId = $derived($videoChannelOnAir?.id ?? null);

	async function load() {
		lastFetch = Date.now();
		try {
			const { data } = await loadLineup();
			error = null;
			if (pollTimer) clearTimeout(pollTimer);
			pollTimer = null;
			if (data.building && polls < BUILD_POLL_MAX) {
				polls += 1;
				pollTimer = setTimeout(() => void load(), BUILD_POLL_MS);
			}
		} catch (err) {
			// Keep showing a lineup we already have; only an empty dial errors.
			if (!$channelLineup) error = err;
		}
	}

	// Time moves the dial on by itself; when a channel's "now" has run out the
	// next is shown from the old lineup, and a fresh one is fetched quietly.
	$effect(() => {
		const now = $airTick;
		if (!lineup) return;
		const stale = [...slots.values()].some((slot) => slot.stale);
		if ((stale && now - lastFetch > STALE_REFETCH_MS) || now - lastFetch > REFRESH_MS) void load();
	});

	async function tune(channel: Pick<VideoChannel, 'id' | 'title'> & { locked?: boolean }) {
		if (channel.locked || tuning) return;
		tuning = channel.id;
		try {
			await tuneChannel(channel);
		} finally {
			tuning = null;
		}
	}

	async function changeExplore(mode: ExploreMode) {
		if (!(await setExploreMode(mode))) {
			showToast('Could not save that. Try again in a moment.', 'error', 3200);
			return;
		}
		// The rings change what the channels pick: refresh what is on.
		void load();
	}

	/** Deep links: /videos?channel= (the Atlas's "Tune in") tunes that
	 *  channel. The old ones still land here: ?videoId= moves to the watch
	 *  page, ?mixId= / ?playlistId= play the collection. The address is cleaned
	 *  first so Back from the watch page returns to the dial instead of
	 *  replaying the link. */
	async function handleDeepLink() {
		const params = new URLSearchParams(window.location.search);
		const channelId = params.get('channel');
		const videoId = params.get('videoId');
		const mixId = params.get('mixId');
		const playlistId = params.get('playlistId');
		if (videoId) {
			await goto(watchUrl(videoId, {
				radio: params.get('radio') === '1',
				title: params.get('title'),
				artistId: Number(params.get('artistId')) || null,
				artistName: params.get('artistName'),
			}), { replaceState: true });
			return;
		}
		if (!channelId && !mixId && !playlistId) return;
		await goto('/videos', { replaceState: true, keepFocus: true });
		if (channelId) {
			// The lineup names the channel for the toasts; tuning does not need it.
			const lineupNow = await ensureLineup();
			const known = lineupNow?.data.channels.find((channel) => channel.id === channelId);
			void tune(known ?? { id: channelId, title: 'this channel' });
		} else if (mixId) void playVideoCollection('mix', mixId);
		else if (playlistId) void playVideoCollection('playlist', playlistId);
	}

	onMount(() => {
		void loadExploreMode();
		void load();
		void handleDeepLink();
	});

	onDestroy(() => {
		if (pollTimer) clearTimeout(pollTimer);
	});
</script>

<svelte:head>
	<title>On Air - NOOR</title>
</svelte:head>

<div class="onair-page">
	<div class="dial-bar">
		<p class="status" aria-live="off">
			<span class="dot" aria-hidden="true"></span>
			<span class="t-label">On air</span>
			<span class="clock">{clock}</span>
			{#if channels.length > 0}<span class="count">{channels.filter((channel) => !channel.locked).length} channels</span>{/if}
		</p>
		<div class="explore" title="How far from your taste the channels roam">
			<Segmented options={EXPLORE_OPTIONS} value={$exploreMode} label="Explore" onchange={(mode) => void changeExplore(mode)} />
		</div>
	</div>

	{#if error && channels.length === 0}
		<ErrorState title="On Air could not load" {error} onretry={() => void load()}>
			{#snippet actions()}
				<a class="btn btn-secondary" href="/videos/stations">Open stations</a>
			{/snippet}
		</ErrorState>
	{:else if !lineup}
		<GuidePlaceholder />
	{:else if channels.length === 0 && building}
		<div class="building">
			<p class="t-body">Programming today's channels. They appear here as each one is ready.</p>
			<Skeleton rows={4} label="Programming today's channels" />
		</div>
	{:else if channels.length === 0}
		<EmptyState
			title="No channels yet"
			copy="Channels open once video discovery has found enough music videos around your taste."
		>
			{#snippet actions()}
				<a class="btn btn-secondary" href="/settings?setting=video-discovery">Video discovery settings</a>
				<a class="btn btn-ghost" href="/videos/stations">Open stations</a>
			{/snippet}
		</EmptyState>
	{:else}
		{#if tonight.length > 0}
			<section class="tonight" aria-labelledby="tonight-label">
				<h3 id="tonight-label" class="t-label">For you tonight</h3>
				<div class="strip" data-count={tonight.length}>
					{#each tonight as channel, index (channel.id)}
						{@const slot = slots.get(channel.id)}
						{#if slot}
							<ChannelCard {channel} {slot} onAir={onAirId === channel.id} busy={tuning === channel.id} rise={index} ontune={(picked) => void tune(picked)} />
						{/if}
					{/each}
				</div>
			</section>
		{/if}

		<section class="channels" aria-labelledby="channels-label">
			<h3 id="channels-label" class="t-label">All channels</h3>
			<div class="rows">
				{#each dial as channel, index (channel.id)}
					{@const slot = slots.get(channel.id)}
					{#if channel.id === firstScene && index > 0}
						<h4 class="family">Scenes</h4>
					{/if}
					{#if slot}
						<ChannelRow {channel} {slot} onAir={onAirId === channel.id} busy={tuning === channel.id} rise={index % 12} ontune={(picked) => void tune(picked)} />
					{/if}
				{/each}
			</div>
			{#if building}
				<p class="note">Still programming a few channels. They join the dial as they are ready.</p>
			{/if}
		</section>
	{/if}

	<p class="elsewhere">
		Prefer a station that only plays new finds? <a href="/videos/stations">Video stations</a>
	</p>
</div>

<style>
	/* Width, gutters and the bottom inset come from the video layout. */
	.onair-page {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: var(--space-6);
	}

	/* A quiet status line over the dial: the live clock and the explore
	   control, on the ground, no box. */
	.dial-bar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		padding: 0 12px;
	}
	.status {
		display: flex;
		align-items: baseline;
		gap: 10px;
		margin: 0;
	}
	.dot {
		align-self: center;
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--state-error);
		box-shadow: 0 0 0 3px color-mix(in srgb, var(--state-error) 22%, transparent);
		animation: on-air-pulse 2.4s var(--ease-standard) infinite;
	}
	@keyframes on-air-pulse {
		0%, 100% { opacity: 1; }
		50% { opacity: 0.45; }
	}
	.clock {
		font-family: var(--font-mono);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		font-variant-numeric: tabular-nums;
		color: var(--text-primary);
	}
	.count {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.tonight {
		container-type: inline-size;
	}
	.tonight,
	.channels {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: var(--group-gap);
		min-width: 0;
	}
	.tonight > .t-label,
	.channels > .t-label {
		margin: 0;
		padding: 0 12px;
	}

	/* Three or four big screens across; two on a narrow window. */
	.strip {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: var(--gap) var(--gap);
		padding: 0 12px;
	}
	.strip[data-count='3'] { grid-template-columns: repeat(3, minmax(0, 1fr)); }
	.strip[data-count='2'] { grid-template-columns: repeat(2, minmax(0, 1fr)); }
	.strip[data-count='1'] { grid-template-columns: minmax(0, 560px); }

	.rows {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: var(--row-gap);
	}
	.family {
		margin: var(--space-4) 0 var(--space-1);
		padding: 0 12px;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
	}

	.building {
		display: grid;
		gap: var(--space-3);
		padding: 0 12px;
	}
	.building p { margin: 0; color: var(--text-secondary); }

	.note,
	.elsewhere {
		margin: 0;
		padding: 0 12px;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}
	.elsewhere a {
		color: var(--text-secondary);
		font-weight: var(--font-weight-semibold);
	}
	.elsewhere a:hover { color: var(--text-primary); }

	/* Sized from the strip's own width (side panels change it): four big
	   screens when they stay big, three when four would shrink them, two
	   then one on narrow windows. The strip never wraps a lone fourth. */
	@container (max-width: 1180px) {
		.strip,
		.strip[data-count] { grid-template-columns: repeat(3, minmax(0, 1fr)); }
		.strip > :global(:nth-child(n + 4)) { display: none; }
	}
	@container (max-width: 760px) {
		.strip,
		.strip[data-count] { grid-template-columns: repeat(2, minmax(0, 1fr)); }
		.strip > :global(:nth-child(n + 3)) { display: none; }
	}
	@container (max-width: 460px) {
		.strip,
		.strip[data-count] { grid-template-columns: minmax(0, 1fr); }
	}
	@media (prefers-reduced-motion: reduce) {
		.dot { animation: none; }
	}
	:global(:root[data-motion='reduce']) .dot { animation: none; }
</style>
