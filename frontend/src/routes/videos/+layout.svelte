<script lang="ts">
	import type { Snippet } from 'svelte';
	import { get } from 'svelte/store';
	import { afterNavigate, goto } from '$app/navigation';
	import { page } from '$app/state';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import VideoNavigation from '$lib/components/video/VideoNavigation.svelte';
	import VideoSearchResults from '$lib/components/video/VideoSearchResults.svelte';
	import { goBack } from '$lib/navigation/back';
	import { videoStageReveal } from '$lib/stores/video_session';
	import { WATCH_PATH, videoSectionQuery, videoTabFor } from '$lib/video/section';

	// One frame for the whole video section, so every tab has the same top:
	// Back and the search field on the first row (the same height as /search),
	// the tab pills under it. The tabs are for browsing; picking a video on
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
	afterNavigate(({ from, to }) => {
		if (from?.url.pathname === to?.url.pathname) return;
		if (carryQuery) carryQuery = false;
		else videoSectionQuery.set('');
	});

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
	<header class="video-header">
		<div class="search-row">
			<button
				type="button"
				class="back-link"
				onclick={() => goBack(onWatchPage ? '/videos' : '/')}
			>Back</button>
			<div class="search-slot">
				<SearchField
					bind:value={$videoSectionQuery}
					variant="page"
					placeholder={tab === 'liked' ? 'Search your liked videos' : 'Search TIDAL videos'}
					ariaLabel={tab === 'liked' ? 'Search your liked videos' : 'Search TIDAL videos'}
					suppressSuggestions
				/>
			</div>
		</div>
		<VideoNavigation current={tab} />
		{#if tab === 'liked' && query}
			<p class="scope-note">
				Filtering your liked videos.
				<button type="button" class="link-btn" onclick={searchAllOfTidal}>Search all of TIDAL for "{query}"</button>
			</p>
		{/if}
	</header>

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
		gap: 28px;
		padding: 0 4px max(var(--bottom-player-height, 0px), 44px, var(--safe-bottom));
	}

	.video-header {
		display: grid;
		gap: var(--space-4);
		width: 100%;
	}

	/* Back sits beside the field instead of on a row of its own, so the field
	   lands at the same height as on /search. The side columns are equal, which
	   keeps the field centered on the page. */
	.search-row {
		display: grid;
		grid-template-columns: 1fr minmax(0, 720px) 1fr;
		align-items: center;
		gap: var(--space-3);
	}

	.search-row .back-link {
		justify-self: start;
	}

	.search-slot {
		min-width: 0;
	}

	.section-body {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		min-width: 0;
		animation: body-in 0.2s ease both;
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

		.search-row {
			grid-template-columns: auto minmax(0, 1fr);
		}
	}
</style>
