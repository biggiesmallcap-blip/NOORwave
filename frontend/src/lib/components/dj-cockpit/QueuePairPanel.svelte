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
		if (deck?.profile_status === 'decode_failed') return 'Analysis failed';
		if (deck?.profile_status === 'retrying') return retryLabel(deck);
		if (deck?.profile_status === 'analyzing') return 'Analyzing';
		if (!deck?.profile_ready) return 'Profile missing';
		if (deck.profile_confidence == null) return 'Profile ready';
		return `${Math.round(deck.profile_confidence * 100)}% confidence`;
	}

	function retryLabel(deck: DjDeckStatus) {
		const wait = formatRetryWait(deck.profile_retry_after_ms);
		if (!wait) return 'Retrying analysis';
		return `Retrying analysis in ${wait}`;
	}

	function formatRetryWait(ms?: number) {
		if (ms == null || ms <= 0) return null;
		const seconds = Math.ceil(ms / 1000);
		if (seconds < 60) return `${seconds}s`;
		const minutes = Math.ceil(seconds / 60);
		return `${minutes}m`;
	}

	function retryReasonLabel(deck?: DjDeckStatus) {
		if (!deck?.profile_retry_reason) return null;
		if (deck.profile_retry_reason === 'asset_not_ready') return 'TIDAL asset unavailable';
		if (deck.profile_retry_reason === 'dash_prebuffer') return 'DASH prebuffer retry';
		if (deck.profile_retry_reason === 'timeout') return 'Analysis timeout';
		return 'Transient analysis failure';
	}

	function passiveAnalysisLabel(deck?: DjDeckStatus) {
		if (deck?.passive_analysis_status === 'retrying') return 'Passive DSP retrying';
		if (deck?.passive_analysis_status === 'skipped') return 'Passive DSP skipped';
		return null;
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
						<span
							class:ready={item.deck.profile_ready}
							class:failed={item.deck.profile_status === 'decode_failed'}
						>
							{confidenceLabel(item.deck)}
						</span>
						{#if item.deck.safe_crossfade_only}
							<span class="safe-only">Safe only</span>
						{/if}
						{#if passiveAnalysisLabel(item.deck)}
							<span class="passive-analysis">{passiveAnalysisLabel(item.deck)}</span>
						{/if}
						{#if item.deck.profile_error}
							<span class="error-detail" title={item.deck.profile_error}>{item.deck.profile_error}</span>
						{/if}
						{#if retryReasonLabel(item.deck)}
							<span class="error-detail">{retryReasonLabel(item.deck)}</span>
						{/if}
						{#if item.deck.passive_analysis_reason}
							<span class="error-detail" title={item.deck.passive_analysis_reason}>
								{item.deck.passive_analysis_reason}
							</span>
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

	.deck-status .failed {
		border-color: color-mix(in srgb, var(--state-error) 46%, transparent);
		color: var(--state-error);
	}

	.deck-status .error-detail {
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.deck-status .safe-only {
		border-color: color-mix(in srgb, var(--state-warning) 42%, transparent);
		color: var(--state-warning);
	}

	.deck-status .passive-analysis {
		border-color: color-mix(in srgb, var(--state-warning) 34%, transparent);
		color: var(--text-secondary);
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
