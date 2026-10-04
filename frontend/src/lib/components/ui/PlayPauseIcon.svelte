<script lang="ts">
	let { playing }: { playing: boolean } = $props();
</script>

<svg class="play-pause-icon" width="24" height="24" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
	{#if playing}
		<rect x="7" y="5" width="4" height="14" rx="0.7" />
		<rect x="13" y="5" width="4" height="14" rx="0.7" />
	{:else}
		<!-- The triangle's visual weight is centered on the same axis as pause. -->
		<path d="M8 4 20 12 8 20Z" />
	{/if}
</svg>

<style>
	.play-pause-icon {
		display: block;
		flex-shrink: 0;
		fill: currentColor;
	}
</style>
