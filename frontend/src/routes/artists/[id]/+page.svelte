<script lang="ts">
	import { page } from '$app/state';
	import type { Snapshot } from './$types';
	import ArtistDetail from '../ArtistDetail.svelte';
	import { captureScroll, restoreScroll } from '$lib/navigation/scroll';

	let artistId = $derived(Number(page.params.id));

	// Phase 5B: back/forward state via SvelteKit snapshot. Snapshot must live on
	// the route's +page.svelte (not the shared component); scroll capture/restore
	// operate on window scroll, independent of which artist view is mounted.
	export const snapshot: Snapshot<{ scrollY: number }> = {
		capture: () => ({ scrollY: captureScroll() }),
		restore: (saved) => {
			restoreScroll(saved.scrollY);
		}
	};
</script>

<ArtistDetail source={{ kind: 'local', artistId }} />
