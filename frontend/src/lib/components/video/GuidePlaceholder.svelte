<script lang="ts">
	// Holds a video guide's shape while it loads: a featured block and rows
	// the same height as real ones, so the content lands without a jump.
	let { rows = 6, feature = true }: { rows?: number; feature?: boolean } = $props();
</script>

<div class="guide-placeholder" aria-busy="true" aria-label="Loading">
	{#if feature}<div class="feature-block"></div>{/if}
	{#each Array(rows) as _, index (index)}
		<div class="row"><span></span><span></span></div>
	{/each}
</div>

<style>
	.guide-placeholder {
		display: grid;
		gap: 2px;
	}
	.feature-block,
	.row span {
		border-radius: 8px;
		background: var(--bg-raised);
		animation: placeholder-pulse 1.4s ease-in-out infinite;
	}
	.feature-block {
		height: clamp(220px, 22vw, 320px);
		margin-bottom: 16px;
		border-radius: 16px;
	}
	.row {
		display: grid;
		grid-template-columns: 220px minmax(0, 1fr);
		gap: 18px;
		padding: 10px 12px;
	}
	.row span {
		height: 76px;
	}
	.row span:first-child {
		height: 44px;
		align-self: center;
	}
	@keyframes placeholder-pulse {
		50% { opacity: 0.55; }
	}
	@media (max-width: 860px) {
		.row { grid-template-columns: minmax(0, 1fr); }
	}
</style>
