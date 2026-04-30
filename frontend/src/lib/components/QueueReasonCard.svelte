<script lang="ts">
	import { parseReason, formatJaccardPct, formatAffinityDelta } from '$lib/utils/reason';

	let {
		reason = null,
		mouseX = 0,
		mouseY = 0,
	}: {
		reason?: string | null;
		mouseX?: number;
		mouseY?: number;
	} = $props();

	const CARD_WIDTH = 240;
	const CARD_OFFSET = 12;

	let cardEl: HTMLDivElement | null = $state(null);
	let measuredHeight = $state(80);

	$effect(() => {
		if (cardEl) {
			measuredHeight = cardEl.offsetHeight;
		}
	});

	let parsed = $derived(parseReason(reason));

	let placement = $derived.by(() => {
		if (!parsed) return { left: 0, top: 0 };
		const cardHeight = measuredHeight;
		const vw = typeof window !== 'undefined' ? window.innerWidth : 1024;
		const vh = typeof window !== 'undefined' ? window.innerHeight : 768;

		let left = mouseX + CARD_OFFSET;
		let top = mouseY - cardHeight - CARD_OFFSET;

		if (top < 8) top = mouseY + CARD_OFFSET;
		if (left + CARD_WIDTH > vw - 8) left = mouseX - CARD_WIDTH - CARD_OFFSET;
		left = Math.max(8, left);
		if (top + cardHeight > vh - 8) top = vh - cardHeight - 8;
		top = Math.max(8, top);

		return { left, top };
	});

	let jaccardLabel = $derived(formatJaccardPct(parsed?.genre_jaccard));
	let affinityLabel = $derived(formatAffinityDelta(parsed?.affinity_mult));
	let hasBreakdown = $derived(jaccardLabel !== null || affinityLabel !== null);
</script>

{#if parsed}
	<div
		bind:this={cardEl}
		class="reason-card"
		style="left: {placement.left}px; top: {placement.top}px; width: {CARD_WIDTH}px"
	>
		<div class="header">Why is this here?</div>
		{#if parsed.prefix}
			<div class="prefix">{parsed.prefix}</div>
		{/if}
		{#if hasBreakdown}
			<div class="rows">
				{#if jaccardLabel !== null}
					<div class="row">
						<span class="row-label">Genre overlap</span>
						<span class="row-value">{jaccardLabel}</span>
					</div>
				{/if}
				{#if affinityLabel !== null}
					<div class="row">
						<span class="row-label">Affinity boost</span>
						<span class="row-value">{affinityLabel}</span>
					</div>
				{/if}
			</div>
		{/if}
	</div>
{/if}

<style>
	.reason-card {
		position: fixed;
		background: rgba(13, 13, 26, 0.95);
		backdrop-filter: blur(8px);
		border: 1px solid #3a3a5c;
		border-radius: 8px;
		padding: 10px 12px;
		z-index: 100;
		box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5);
		pointer-events: none;
		font-family: inherit;
		color: #e8e8f0;
		animation: reason-card-fade-in 100ms ease-out;
	}

	@keyframes reason-card-fade-in {
		from { opacity: 0; transform: translateY(2px); }
		to { opacity: 1; transform: translateY(0); }
	}

	.header {
		font-size: 10px;
		font-weight: 700;
		letter-spacing: 0.06em;
		color: #9999b8;
		text-transform: uppercase;
		margin-bottom: 6px;
	}

	.prefix {
		font-size: 12px;
		line-height: 1.4;
		color: #c8c8dc;
		word-break: break-word;
	}

	.rows {
		margin-top: 8px;
		padding-top: 8px;
		border-top: 1px solid rgba(58, 58, 92, 0.6);
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.row {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		font-size: 11px;
	}

	.row-label {
		color: #9999b8;
	}

	.row-value {
		color: #e8e8f0;
		font-variant-numeric: tabular-nums;
	}
</style>
