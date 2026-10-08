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

	// One frame for the whole video section, so every tab has the same top:
	// the shared command header puts Back and the search field on the first
	// row (the same height as /search) and the tab pills under it. The tabs are for browsing; picking a video on
	// any of them opens the watch page, the only place the big player lives.

	let { children }: { children: Snippet } = $props();

	let tab = $derived(videoTabFor(page.url.pathname));
	let onWatchPage = $derived(page.url.pathname === WATCH_PATH);
	let query = $derived($videoSectionQuery.trim());
	// Liked filters its own wall with the field; everywhere else the field
	// searches TIDAL and the results take the page while it has text.
	let searchingTidal = $derived(query.length > 0 && tab !== 'liked');

	// A new page starts with an empty field, except when the Liked tab hands
	// its filter over to a full TIDAL search.
	let carryQuery = false;
	afterNavigate(({ from, to, type }) => {
		if (from?.url.pathname === to?.url.pathname) return;
		if (carryQuery) carryQuery = false;
		else videoSectionQuery.set('');
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
				placeholder={tab === 'liked' ? 'Search your liked videos' : 'Search TIDAL videos'}
				ariaLabel={tab === 'liked' ? 'Search your liked videos' : 'Search TIDAL videos'}
				suppressSuggestions
			/>
		{/snippet}
		{#snippet tabs()}
			<!-- No tab is lit on the watch page. Tab hops replace the history
			     entry, so Back leaves the section in one step. -->
			<ScopeTabs tabs={VIDEO_TABS} current={tab} label="Video pages" replaceState />
		{/snippet}
		{#if tab === 'liked' && query}
			<p class="scope-note">
				Filtering your liked videos.
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
