<script lang="ts">
	import { setVideoBrowseMode, videoBrowseMode, videoSession } from '$lib/stores/video_session';

	// The video pages' back button. Same element, class and place as every
	// other page's (`<button class="back-link">Back</button>` first, top left);
	// only the destination differs: it returns to the playing video instead of
	// stepping back through history. The accessible name says where it goes and
	// still starts with the visible "Back". Renders nothing when no video is
	// playing: the video pages are siblings, reached by the tabs, not a stack.
	let { current, canBrowse = false }: {
		current: 'videos' | 'liked' | 'stations' | 'editorial';
		canBrowse?: boolean;
	} = $props();

	let target = $derived.by(() => {
		if (!$videoSession.active) return null;
		if (current !== 'videos') return 'player-page';
		if ($videoBrowseMode) return 'player';
		return canBrowse ? 'picks' : null;
	});
</script>

{#if target === 'picks'}
	<button type="button" class="back-link" aria-label="Back to picks" onclick={() => setVideoBrowseMode(true)}>Back</button>
{:else if target === 'player'}
	<button type="button" class="back-link" aria-label="Back to the player" onclick={() => setVideoBrowseMode(false)}>Back</button>
{:else if target === 'player-page'}
	<a class="back-link" href="/videos" aria-label="Back to the player" onclick={() => setVideoBrowseMode(false)}>Back</a>
{/if}

<style>
	/* Pages lay out as grid or flex columns; either way the pill keeps its
	   own width at the left, as on album and artist pages. */
	.back-link {
		justify-self: start;
		align-self: flex-start;
	}
</style>
