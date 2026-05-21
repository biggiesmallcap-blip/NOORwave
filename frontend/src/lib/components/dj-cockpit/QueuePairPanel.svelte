<script lang="ts">
	import type { DjDeckStatus } from '$lib/api/client';

	let {
		current = undefined,
		next = undefined,
	}: {
		current?: DjDeckStatus;
		next?: DjDeckStatus;
	} = $props();

	function confidenceLabel(deck?: DjDeckStatus) {
		if (!deck?.profile_ready) return 'Profile missing';
		if (deck.profile_confidence == null) return 'Profile ready';
		return `${Math.round(deck.profile_confidence * 100)}% confidence`;
	}
</script>

<section class="pair-panel" aria-labelledby="dj-pair-heading">
	<header>
		<div>
			<p class="eyebrow">Queue pair</p>
			<h2 id="dj-pair-heading">Current and next</h2>
		</div>
	</header>

	<div class="deck-grid">
		{#each [{ label: 'Outgoing', deck: current }, { label: 'Incoming', deck: next }] as item}
			<article class="deck-card" class:missing={!item.deck}>
				<span class="deck-label">{item.label}</span>
				{#if item.deck}
					<h3 title={item.deck.title}>{item.deck.title}</h3>
					<p title={item.deck.artist ?? 'Unknown artist'}>{item.deck.artist ?? 'Unknown artist'}</p>
					<div class="deck-status">
						<span class:ready={item.deck.profile_ready}>{confidenceLabel(item.deck)}</span>
						{#if item.deck.safe_crossfade_only}
							<span class="safe-only">Safe only</span>
						{/if}
					</div>
					<dl>
						<div>
							<dt>Beats</dt>
							<dd>{item.deck.beat_count ?? 0}</dd>
						</div>
						<div>
							<dt>Downbeats</dt>
							<dd>{item.deck.downbeat_count ?? 0}</dd>
						</div>
						<div>
							<dt>Phrases</dt>
							<dd>{item.deck.phrase_count ?? 0}</dd>
						</div>
					</dl>
					<small>{item.deck.media_ref_kind}:{item.deck.media_ref_id}</small>
				{:else}
					<h3>No deck</h3>
					<p>The queue pair is not resolved yet.</p>
				{/if}
			</article>
		{/each}
	</div>
</section>

<style>
	.pair-panel {
		display: grid;
		gap: var(--space-3);
	}

	header {
		display: flex;
		justify-content: space-between;
		gap: var(--space-3);
	}

	.eyebrow {
		margin: 0 0 var(--space-1);
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		text-transform: uppercase;
		letter-spacing: 0;
	}

	h2,
	h3,
	p {
		margin: 0;
	}

	h2 {
		font-size: var(--font-size-xl);
		line-height: var(--line-height-tight);
	}

	.deck-grid {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--space-3);
	}

	.deck-card {
		display: grid;
		gap: var(--space-2);
		min-width: 0;
		padding: var(--space-3);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-surface) 86%, transparent);
	}

	.deck-card.missing {
		opacity: 0.72;
	}

	.deck-label,
	small {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-snug);
	}

	h3,
	p {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	h3 {
		font-size: var(--font-size-lg);
		line-height: var(--line-height-tight);
	}

	p {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.deck-status {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-1);
	}

	.deck-status span {
		padding: var(--space-1) var(--space-2);
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		line-height: 1;
	}

	.deck-status .ready {
		border-color: color-mix(in srgb, var(--state-success) 42%, transparent);
		color: var(--state-success);
	}

	.deck-status .safe-only {
		border-color: color-mix(in srgb, var(--state-warning) 42%, transparent);
		color: var(--state-warning);
	}

	dl {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: var(--space-2);
		margin: 0;
	}

	dl div {
		display: grid;
		gap: var(--space-1);
	}

	dt {
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
		text-transform: uppercase;
		letter-spacing: 0;
	}

	dd {
		margin: 0;
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
	}

	@media (max-width: 760px) {
		.deck-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
