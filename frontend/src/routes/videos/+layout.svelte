<script lang="ts">
	import type { Snippet } from 'svelte';
	import { get } from 'svelte/store';
	import { afterNavigate, goto } from '$app/navigation';
	import { page } from '$app/state';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import CommandHeader from '$lib/components/ui/CommandHeader.svelte';
	import ScopeTabs from '$lib/components/ui/ScopeTabs.svelte';
	import VideoSearchResults from '$lib/components/video/VideoSearchResults.svelte';
	import { goBack } from '$lib/navigation/back';
	import { captureScroll, restoreScroll, scrollWorkspaceTop } from '$lib/navigation/scroll';
	import { videoStageReveal } from '$lib/stores/video_session';
	import { VIDEO_TABS, WATCH_PATH, videoSectionQuery, videoTabFor } from '$lib/video/section';
	import { lensesToSearch, parseLenses } from '$lib/video/archive/lenses';
	import { ATLAS_SEARCH_LABEL } from '$lib/video/atlas/links';

	// One frame for the whole video section, so every tab has the same top:
	// the shared command header puts Back and the search field on the first
	// row (the same height as /search) and the tab pills under it. The tabs are for browsing; picking a video on
	// any of them opens the watch page, the only place the big player lives.

	let { children }: { children: Snippet } = $props();

	let tab = $derived(videoTabFor(page.url.pathname));
	let onWatchPage = $derived(page.url.pathname === WATCH_PATH);
	let onLiked = $derived(page.url.pathname === '/videos/liked');
	let onArchive = $derived(page.url.pathname === '/videos/archive');
	let onAtlas = $derived(page.url.pathname === '/videos/atlas');
	let query = $derived($videoSectionQuery.trim());
	// Liked filters its own wall, Archive filters song titles (the q lens) and
	// the Atlas finds an artist on the map; everywhere else the field searches
	// TIDAL and the results take the page while it has text.
	let filtersInPlace = $derived(onLiked || onArchive || onAtlas);
	let searchingTidal = $derived(query.length > 0 && !filtersInPlace);
	let placeholder = $derived(
		onLiked ? 'Search your liked videos'
			: onArchive ? 'Search song titles in the archive'
				: onAtlas ? ATLAS_SEARCH_LABEL
					: 'Search TIDAL videos'
	);

	// Archive: the field is the title lens. Typing writes ?q= after a short
	// pause (replacing the entry, so Back leaves the Archive in one step); a
	// navigation that changes ?q= (Show everything, a deep link) writes it
	// back into the field unless a keystroke is still waiting to be sent.
	let archiveTimer: ReturnType<typeof setTimeout> | null = null;
	$effect(() => {
		if (!onArchive) return;
		const q = query;
		if (q === (page.url.searchParams.get('q') ?? '')) return;
		if (archiveTimer) clearTimeout(archiveTimer);
		archiveTimer = setTimeout(() => {
			archiveTimer = null;
			const next = { ...parseLenses(page.url.searchParams), q };
			void goto(`/videos/archive${lensesToSearch(next)}`, { replaceState: true, keepFocus: true, noScroll: true });
		}, 250);
	});

	// A new page starts with an empty field, except when the Liked tab hands
	// its filter over to a full TIDAL search.
	let carryQuery = false;
	afterNavigate(({ from, to, type }) => {
		const archiveQ = to?.url.pathname === '/videos/archive' ? to.url.searchParams.get('q') ?? '' : null;
		if (from?.url.pathname === to?.url.pathname) {
			if (archiveQ !== null && !archiveTimer && archiveQ !== get(videoSectionQuery).trim()) videoSectionQuery.set(archiveQ);
			return;
		}
		if (carryQuery) carryQuery = false;
		else videoSectionQuery.set(archiveQ ?? '');
		// The app scrolls main.workspace, which SvelteKit's reset never
		// touches: a pick halfway down a tab would open the watch page halfway
		// down. Forward moves start at the top; Back/Forward restore below.
		if (type !== 'popstate') scrollWorkspaceTop();
	});

	// Per history entry: Back from the watch page lands on the tab at the
	// tile you picked from.
	export const snapshot = {
		capture: () => captureScroll(),
		restore: (top: number) => restoreScroll(top),
	};

	function searchAllOfTidal() {
		carryQuery = true;
		void goto('/videos');
	}

	// Every pick (any tab, search, a station, the queue) opens the watch page.
	// Queue steps don't bump this, so autoplay never pulls you off a tab.
	let handledReveal = get(videoStageReveal);
	$effect(() => {
		const nonce = $videoStageReveal;
		if (nonce === handledReveal) return;
		handledReveal = nonce;
		videoSectionQuery.set('');
		if (window.location.pathname !== WATCH_PATH) void goto(WATCH_PATH);
	});
</script>

<div class="video-section">
	<CommandHeader onback={() => goBack(onWatchPage ? '/videos' : '/')}>
		{#snippet field()}
			<SearchField
				bind:value={$videoSectionQuery}
				variant="page"
				{placeholder}
				ariaLabel={placeholder}
				suppressSuggestions
			/>
		{/snippet}
		{#snippet tabs()}
			<!-- No tab is lit on the watch page. Tab hops replace the history
			     entry, so Back leaves the section in one step. -->
			<ScopeTabs tabs={VIDEO_TABS} current={tab} label="Video pages" replaceState />
		{/snippet}
		{#if (onLiked || onArchive) && query}
			<p class="scope-note">
				{onLiked ? 'Filtering your liked videos.' : 'Filtering the archive by title.'}
				<button type="button" class="link-btn" onclick={searchAllOfTidal}>Search all of TIDAL for "{query}"</button>
			</p>
		{/if}
	</CommandHeader>

	{#if searchingTidal}
		<VideoSearchResults />
	{/if}
	<!-- Hidden, not unmounted: clearing the field returns to the same tab at
	     the same tile, and a hidden watch stage drops the player to the
	     corner (the dock reads its zero size) instead of covering results. -->
	<div class="section-body" hidden={searchingTidal}>
		{@render children()}
	</div>
</div>

<style>
	/* Same frame as /search: content width, centered, small side gutter. */
	.video-section {
		width: min(100%, var(--content-width));
		margin: 0 auto;
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: var(--header-gap);
		padding: 0 4px max(var(--bottom-player-height, 0px), 44px, var(--safe-bottom));
	}

	.section-body {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		min-width: 0;
		animation: body-in var(--motion-base) both;
	}

	.section-body[hidden] {
		display: none;
	}

	@keyframes body-in {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	.scope-note {
		margin: 0;
		text-align: center;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.link-btn {
		margin-left: var(--space-1);
		color: var(--accent-strong);
		font-weight: var(--font-weight-semibold);
	}

	@media (max-width: 620px) {
		.video-section {
			gap: 20px;
		}
	}
</style>
