<script lang="ts">
	import { REASON_LABELS, REASON_EXPLANATIONS, SOURCE_LABELS } from './discover_space_story';
	import type { DiscoverTrackNode } from './discover_space_types';

	interface Props {
		node: DiscoverTrackNode | null;
		mouseX: number;
		mouseY: number;
		seedTrackId: number | null;
		isLocked: boolean;
	}
	let { node, mouseX, mouseY, seedTrackId, isLocked }: Props = $props();

	let cardEl: HTMLDivElement | undefined = $state();

	// Keep card inside viewport
	let style = $derived.by(() => {
		if (!node) return '';
		const margin = 12;
		const cardW = 260;
		const cardH = 180;
		let left = mouseX + margin;
		let top = mouseY - cardH / 2;
		if (typeof window !== 'undefined') {
			if (left + cardW > window.innerWidth) left = mouseX - cardW - margin;
			if (top < 8) top = 8;
			if (top + cardH > window.innerHeight) top = window.innerHeight - cardH - 8;
		}
		return `left:${left}px; top:${top}px;`;
	});
</script>

{#if node}
	<div
		bind:this={cardEl}
		class="hover-card"
		style={style}
		role="tooltip"
		aria-label="{node.title} details"
	>
		<div class="card-header">
			<div class="card-title">{node.title}</div>
			<div class="card-artist">{node.artist}</div>
		</div>

		<div class="card-chips">
			{#if node.energy != null}
				<span class="chip energy" style:--e={node.energy}>⚡ {Math.round(node.energy * 100)}%</span>
			{/if}
			{#if node.danceability != null}
				<span class="chip">💃 {Math.round(node.danceability * 100)}%</span>
			{/if}
			{#if node.bpm != null}
				<span class="chip">♩ {Math.round(node.bpm)} bpm</span>
			{/if}
			{#if node.camelotKey}
				<span class="chip">{node.camelotKey}</span>
			{/if}
		</div>

		<div class="card-reason">
			<span class="reason-badge">{REASON_LABELS[node.primaryReason]}</span>
			<span class="reason-copy">{REASON_EXPLANATIONS[node.primaryReason]}</span>
		</div>

		<div class="card-meta">
			<span class="source-tag">{SOURCE_LABELS[node.source]}</span>
			{#if node.isSeed}<span class="seed-tag">{isLocked ? '🔒 Locked seed' : '▶ Auto-seed'}</span>{/if}
			{#if node.isColdStart}<span class="cold-tag">Cold start</span>{/if}
			{#if node.topGenre}<span class="genre-tag">{node.topGenre}</span>{/if}
		</div>

		<div class="card-conf">
			<div class="conf-bar" style:--c={node.confidence}></div>
			<span class="conf-label">{Math.round(node.confidence * 100)}% confidence</span>
		</div>
	</div>
{/if}

<style>
	.hover-card {
		position: fixed;
		z-index: 200;
		width: 260px;
		background: rgba(12, 12, 24, 0.96);
		backdrop-filter: blur(12px);
		border: 1px solid rgba(255, 255, 255, 0.1);
		border-radius: 12px;
		padding: 12px 14px;
		display: flex;
		flex-direction: column;
		gap: 8px;
		pointer-events: none;
		box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
	}
	.card-title {
		font-weight: 600;
		font-size: 0.88rem;
		color: rgba(255, 255, 255, 0.95);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.card-artist { font-size: 0.78rem; color: rgba(255, 255, 255, 0.5); }

	.card-chips {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
	}
	.chip {
		padding: 2px 7px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.07);
		color: rgba(255, 255, 255, 0.7);
		font-size: 0.68rem;
		font-variant-numeric: tabular-nums;
	}
	.chip.energy {
		background: hsla(calc(240deg - var(--e, 0.5) * 200deg), 60%, 40%, 0.3);
		color: hsla(calc(240deg - var(--e, 0.5) * 200deg), 80%, 80%, 1);
	}

	.card-reason { display: flex; flex-direction: column; gap: 2px; }
	.reason-badge {
		font-size: 0.7rem;
		font-weight: 600;
		color: rgba(124, 128, 255, 0.9);
		text-transform: uppercase;
		letter-spacing: 0.06em;
	}
	.reason-copy { font-size: 0.72rem; color: rgba(255, 255, 255, 0.4); line-height: 1.4; }

	.card-meta {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
	}
	.source-tag, .seed-tag, .cold-tag, .genre-tag {
		padding: 1px 6px;
		border-radius: 4px;
		font-size: 0.65rem;
	}
	.source-tag { background: rgba(100,120,220,0.15); color: rgba(160,170,255,0.8); }
	.seed-tag { background: rgba(80,80,200,0.2); color: rgba(180,180,255,0.9); }
	.cold-tag { background: rgba(60,60,80,0.3); color: rgba(160,160,180,0.6); }
	.genre-tag { background: rgba(80,180,100,0.1); color: rgba(120,200,140,0.8); }

	.card-conf {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.conf-bar {
		flex: 1;
		height: 3px;
		border-radius: 999px;
		background: rgba(255,255,255,0.08);
		position: relative;
		overflow: hidden;
	}
	.conf-bar::after {
		content: '';
		position: absolute;
		left: 0; top: 0; bottom: 0;
		width: calc(var(--c, 0.5) * 100%);
		background: rgba(124, 128, 255, 0.7);
		border-radius: 999px;
	}
	.conf-label { font-size: 0.65rem; color: rgba(255,255,255,0.35); white-space: nowrap; }
</style>
