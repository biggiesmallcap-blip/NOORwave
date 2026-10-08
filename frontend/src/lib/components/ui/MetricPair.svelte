<script lang="ts">
	let {
		label,
		value,
		copy = ''
	}: {
		label: string;
		value: string | number;
		copy?: string;
	} = $props();
</script>

<!-- A stat on the ground: never a bordered tile (STYLING.md "Boundaries"). -->
<div class="metric-pair">
	<span class="label">{label}</span>
	<strong>{value}</strong>
	{#if copy}
		<p>{copy}</p>
	{/if}
</div>

<style>
	.metric-pair {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.label {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	strong {
		font-family: var(--font-body);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-bold);
		font-variant-numeric: tabular-nums;
		line-height: var(--line-height-tight);
	}

	p {
		margin: 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}
</style>
