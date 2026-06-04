<script lang="ts">
	import TidalEditorialPage from '$lib/components/tidal/TidalEditorialPage.svelte';
</script>

<TidalEditorialPage
	pagePath="hires"
	title="HiRes"
	subtitle="High-resolution releases and editorial picks from TIDAL."
	emptyText="TIDAL returned no HiRes modules right now."
	disconnectedText="Connect TIDAL to browse HiRes."
	errorText="Could not load HiRes."
/>
