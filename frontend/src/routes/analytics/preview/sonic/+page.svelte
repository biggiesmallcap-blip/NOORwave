<script lang="ts">
	/**
	 * Dev-only preview route for SonicField. Gated by import.meta.env.DEV.
	 */

	import SonicField from '$lib/components/charts/SonicField.svelte';
	import fixture from '$lib/fixtures/analytics-signals.json';
	import { generateDemoSonicField, type SonicProfile } from '$lib/fixtures/demo-sonic-field';
	import type { SonicView } from '$lib/api/client';

	type DataSource = 'captured' | SonicProfile;
	let dataSource = $state<DataSource>('club');

	const captured = (fixture as { signals: any }).signals.sonic_field as SonicView;
	const field = $derived<SonicView>(
		dataSource === 'captured' ? captured : generateDemoSonicField(dataSource),
	);

	// No callback overrides — let the component fall through to the real
	// `playTrackNow` + `buildTrackMenu` wiring so the preview matches production
	// behaviour. Demo track IDs won't resolve to real tracks, so playback won't
	// actually start, but the menu UI renders exactly as it will on the live page.
</script>

{#if import.meta.env.DEV}
	<div class="preview">
		<header>
			<h1>Sonic field — preview</h1>
			<p class="subtitle">Visual tuning sandbox for <code>SonicField.svelte</code>. Dev-only route.</p>
		</header>

		<div class="controls">
			<label>
				<span class="control-label">Data</span>
				<select bind:value={dataSource}>
					<option value="captured">Captured (real DB)</option>
					<option value="club">Demo · Club (upper-right cluster)</option>
					<option value="eclectic">Demo · Eclectic spread (5 clusters)</option>
					<option value="chill">Demo · Chill (lower-left cluster)</option>
					<option value="aggressive">Demo · Aggressive (bottom-right cluster)</option>
				</select>
			</label>
		</div>

		<section class="card glass">
			<SonicField {field} />
		</section>

		<aside class="info">
			<h2>Source data</h2>
			<dl>
				<dt>Tracks</dt><dd>{field.total}</dd>
				<dt>Coverage</dt><dd>{field.coverage.analyzed} / {field.coverage.total_listened}</dd>
				<dt>Tier opacities</dt><dd class="mono">0.30 · 0.50 · 0.70 · 0.85 · 1.00</dd>
				<dt>Quadrants</dt><dd class="mono">contemplative · euphoric · melancholy · aggressive</dd>
			</dl>
			<p class="hint">
				Right-click any dot to open the universal track menu (<code>buildTrackMenu</code>
				from <code>$lib/player/track_menu</code>) — same Play / Radio / Add to queue /
				Go to artist actions used everywhere else in the app. Click triggers
				<code>playTrackNow</code>; demo track IDs won't resolve to real tracks, so
				playback is a no-op here, but the menu UI is the production wiring.
			</p>
		</aside>
	</div>
{:else}
	<div class="not-found">
		<h1>404</h1>
		<p>This route is dev-only.</p>
	</div>
{/if}

<style>
	.preview {
		max-width: var(--content-width);
		margin: 0 auto;
		padding: var(--space-5) var(--space-5) var(--space-7);
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}

	header h1 {
		font-family: var(--font-display);
		font-size: 1.6rem;
		font-weight: 600;
		margin: 0 0 var(--space-1);
	}

	.subtitle {
		font-family: var(--font-body);
		color: var(--text-secondary);
		margin: 0;
	}

	.controls {
		display: flex;
		gap: var(--space-5);
		align-items: center;
		flex-wrap: wrap;
		padding: var(--space-3) var(--space-4);
		background: var(--bg-elevated);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
	}

	.controls label {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	.control-label {
		font-family: var(--font-mono);
		font-size: 0.65rem;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--text-tertiary);
	}

	.controls select {
		font-family: var(--font-mono);
		font-size: 0.78rem;
		background: var(--input-bg);
		border: 1px solid var(--input-border);
		color: var(--text-primary);
		padding: 4px 8px;
		border-radius: var(--radius-xs);
	}

	/* .glass (from app.css) supplies the surface; .card just adds inner padding. */
	.card {
		padding: var(--space-4);
	}

	.info {
		display: grid;
		grid-template-columns: 1fr;
		gap: var(--space-3);
		padding: var(--space-4);
		background: var(--bg-elevated);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
	}

	.info h2 {
		font-family: var(--font-mono);
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--text-tertiary);
		margin: 0;
	}

	.info dl {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: var(--space-2) var(--space-4);
		margin: 0;
		font-family: var(--font-mono);
		font-size: 0.78rem;
	}

	.info dt {
		color: var(--text-tertiary);
	}

	.info dd {
		margin: 0;
		color: var(--text-primary);
	}

	.info dd.mono {
		font-family: var(--font-mono);
	}

	.info .hint {
		margin: 0;
		font-family: var(--font-body);
		color: var(--text-secondary);
		font-size: 0.85rem;
	}

	.not-found {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		min-height: 60vh;
		gap: var(--space-3);
	}
</style>
