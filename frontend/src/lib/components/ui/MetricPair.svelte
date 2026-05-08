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

<div class="metric-pair glass">
	<span class="label">{label}</span>
	<strong>{value}</strong>
	{#if copy}
		<p>{copy}</p>
	{/if}
</div>

<style>
	.metric-pair {
		padding: 16px;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.label {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}

	strong {
		font-family: var(--font-body);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-bold);
		letter-spacing: -0.01em;
	}

	p {
		color: var(--text-secondary);
	}
</style>
