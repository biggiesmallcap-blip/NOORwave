<script lang="ts">
	let { name = '', active = false }: { name?: string; active?: boolean } = $props();
</script>

<span class="artist-chip" class:active>{name}</span>

<style>
	.artist-chip {
		padding: 4px 10px;
		border-radius: 999px;
		background: rgba(255,255,255,0.05);
		font-size: 0.8rem;
		color: var(--text-secondary, rgba(255,255,255,0.6));
		display: inline-block;
	}
	.artist-chip.active {
		background: rgba(124,128,255,0.2);
		color: var(--text-primary, rgba(255,255,255,0.95));
	}
</style>
