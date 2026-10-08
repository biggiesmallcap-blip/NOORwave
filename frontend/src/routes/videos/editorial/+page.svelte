<script lang="ts">
	import TidalEditorialPage from '$lib/components/tidal/TidalEditorialPage.svelte';
	import { playEditorialItem } from '$lib/video/play_collection';
</script>

<TidalEditorialPage
	pagePath="videos"
	title="TIDAL editorial videos"
	subtitle="Video collections selected by TIDAL."
	emptyText="TIDAL returned no video modules right now."
	disconnectedText="Connect TIDAL to browse editorial videos."
	errorText="Could not load TIDAL videos."
	mediaKind="video"
	embedded
	onItemSelect={playEditorialItem}
/>
