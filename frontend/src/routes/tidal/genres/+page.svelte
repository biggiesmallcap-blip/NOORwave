<script lang="ts">
	import TidalEditorialPage from '$lib/components/tidal/TidalEditorialPage.svelte';
</script>

<TidalEditorialPage
	pagePath="genres"
	title="TIDAL genres"
	subtitle="Browse TIDAL editorial genre shelves separately from your library galaxy."
	emptyText="TIDAL returned no genre modules right now."
	disconnectedText="Connect TIDAL to browse editorial genres."
	errorText="Could not load TIDAL genres."
/>
