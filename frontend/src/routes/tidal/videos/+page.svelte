<script lang="ts">
	import TidalEditorialPage from '$lib/components/tidal/TidalEditorialPage.svelte';
</script>

<TidalEditorialPage
	pagePath="videos"
	title="TIDAL videos"
	subtitle="Browse TIDAL editorial video shelves separately from video search and playback."
	emptyText="TIDAL returned no video modules right now."
	disconnectedText="Connect TIDAL to browse editorial videos."
	errorText="Could not load TIDAL videos."
	backFallback="/videos"
	mediaKind="video"
/>
