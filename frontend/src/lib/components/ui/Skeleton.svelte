<script lang="ts">
	let {
		rows = 3,
		label = 'Loading'
	}: {
		rows?: number;
		label?: string;
	} = $props();
</script>

<div class="skeleton-wrap" role="status" aria-label={label}>
	{#each Array(rows) as _, i (i)}
		<div class="skeleton-row" style="--idx: {i};"></div>
	{/each}
	<span class="visually-hidden">{label}</span>
</div>

<style>
	.skeleton-wrap {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 18px 0;
	}

	.skeleton-row {
		height: 14px;
		border-radius: 6px;
		background: linear-gradient(
			90deg,
			var(--bg-surface, rgba(255, 255, 255, 0.04)) 0%,
			var(--bg-hover, rgba(255, 255, 255, 0.10)) 50%,
			var(--bg-surface, rgba(255, 255, 255, 0.04)) 100%
		);
		background-size: 200% 100%;
		animation: shimmer 1.4s ease-in-out infinite;
		animation-delay: calc(var(--idx) * 80ms);
		opacity: 0.7;
	}

	.skeleton-row:nth-child(odd) { width: 78%; }
	.skeleton-row:nth-child(even) { width: 92%; }

	@keyframes shimmer {
		0% { background-position: 200% 0; }
		100% { background-position: -200% 0; }
	}

	.visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		white-space: nowrap;
		border: 0;
	}

	@media (prefers-reduced-motion: reduce) {
		.skeleton-row { animation: none; }
	}
</style>
