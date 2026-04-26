<script lang="ts">
	import type { DiscoverTrackNode } from './discover.types';

	let {
		node = null,
		mouseX = 0,
		mouseY = 0,
		seedTrackId = null,
		isLocked = false,
	}: {
		node?: DiscoverTrackNode | null;
		mouseX?: number;
		mouseY?: number;
		seedTrackId?: number | null;
		isLocked?: boolean;
	} = $props();

	const CARD_WIDTH = 260;
	const CARD_OFFSET = 12;

	let cardEl: HTMLDivElement | null = $state(null);
	let measuredHeight = $state(130);

	$effect(() => {
		if (cardEl) {
			measuredHeight = cardEl.offsetHeight;
		}
	});

	let isSeed = $derived(node !== null && node.track_id === seedTrackId);

	// Compute placement: above cursor by default, flip below if too close to top.
	// Anchor right edge if too close to viewport right.
	let placement = $derived.by(() => {
		if (!node) return { left: 0, top: 0, anchor: 'top-left' as const };
		const cardHeight = measuredHeight;
		const vw = typeof window !== 'undefined' ? window.innerWidth : 1024;
		const vh = typeof window !== 'undefined' ? window.innerHeight : 768;

		let left = mouseX + CARD_OFFSET;
		let top = mouseY - cardHeight - CARD_OFFSET;

		if (top < 8) top = mouseY + CARD_OFFSET; // flip below
		if (left + CARD_WIDTH > vw - 8) left = mouseX - CARD_WIDTH - CARD_OFFSET; // flip left
		left = Math.max(8, left);
		if (top + cardHeight > vh - 8) top = vh - cardHeight - 8;
		top = Math.max(8, top);

		return { left, top, anchor: 'top-left' as const };
	});

	let chips = $derived.by(() => {
		if (!node) return [] as string[];
		const out: string[] = [];
		if (node.bpm != null) out.push(`${Math.round(node.bpm)} BPM`);
		if (node.camelot_key) out.push(node.camelot_key);
		else if (node.key_signature) out.push(node.key_signature);
		if (node.energy != null) out.push(`${Math.round(node.energy * 100)}% energy`);
		if (node.top_genre) out.push(node.top_genre);
		return out;
	});
</script>

{#if node}
	<div
		bind:this={cardEl}
		class="hover-card"
		style="left: {placement.left}px; top: {placement.top}px; width: {CARD_WIDTH}px"
	>
		{#if isSeed}
			<div class="seed-pill">
				{isLocked ? '🔒 Locked seed' : '▶ Playing'}
			</div>
		{:else if node.source === 'external'}
			<div class="source-tag">EXTERNAL · TIDAL</div>
		{/if}
		<div class="title">{node.title}</div>
		<div class="meta">
			{node.artist_name}{#if node.album_title}<span class="dot"> · </span>{node.album_title}{/if}
		</div>
		{#if chips.length > 0}
			<div class="chips">
				{#each chips as chip}
					<span class="chip">{chip}</span>
				{/each}
			</div>
		{/if}
	</div>
{/if}

<style>
	.hover-card {
		position: fixed;
		background: rgba(13, 13, 26, 0.95);
		backdrop-filter: blur(8px);
		border: 1px solid #3a3a5c;
		border-radius: 8px;
		padding: 12px 14px;
		z-index: 100;
		box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5);
		pointer-events: none;
		font-family: inherit;
		color: #e8e8f0;
		animation: hover-card-fade-in 100ms ease-out;
	}

	@keyframes hover-card-fade-in {
		from { opacity: 0; transform: translateY(2px); }
		to { opacity: 1; transform: translateY(0); }
	}

	.seed-pill {
		display: inline-block;
		background: #5b4ef8;
		color: #fff;
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 1px;
		padding: 3px 8px;
		border-radius: 999px;
		margin-bottom: 8px;
	}

	.source-tag {
		font-size: 10px;
		color: #5b4ef8;
		letter-spacing: 1px;
		font-weight: 600;
		margin-bottom: 6px;
	}

	.title {
		font-size: 14px;
		font-weight: 700;
		margin-bottom: 2px;
		line-height: 1.3;
	}

	.meta {
		font-size: 12px;
		color: #a0a0c0;
		margin-bottom: 10px;
		line-height: 1.4;
	}

	.dot {
		color: #5b5b7a;
	}

	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
	}

	.chip {
		background: #1e1e35;
		border: 1px solid #3a3a5c;
		border-radius: 999px;
		padding: 3px 9px;
		font-size: 10px;
		color: #c0c0d8;
	}
</style>
