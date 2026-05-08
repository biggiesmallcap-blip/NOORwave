<script lang="ts">
	/**
	 * Dev-only preview for AudioProfileStrip — four-up audio character summary.
	 */

	import AudioProfileStrip from '$lib/components/analytics/AudioProfileStrip.svelte';
	import fixture from '$lib/fixtures/analytics-signals.json';
	import type { AudioProfile } from '$lib/api/client';

	type Variant = 'captured' | 'loud' | 'dynamic' | 'bass-heavy' | 'treble-heavy';
	let variant = $state<Variant>('captured');

	const captured = (fixture as { signals: any }).signals.audio_profile as AudioProfile;

	const variants: Record<Variant, AudioProfile> = {
		captured,
		loud: {
			dynamic_range_dr: 5.4,
			loudness_lufs: -7.8,
			bass_tilt: 0.8,
			treble_tilt: -0.8,
			coverage: { analyzed: 412, total_listened: 480 },
		},
		dynamic: {
			dynamic_range_dr: 14.2,
			loudness_lufs: -16.5,
			bass_tilt: -0.4,
			treble_tilt: 0.4,
			coverage: { analyzed: 91, total_listened: 102 },
		},
		'bass-heavy': {
			dynamic_range_dr: 8.7,
			loudness_lufs: -11.2,
			bass_tilt: 4.2,
			treble_tilt: -4.2,
			coverage: { analyzed: 240, total_listened: 312 },
		},
		'treble-heavy': {
			dynamic_range_dr: 9.6,
			loudness_lufs: -10.4,
			bass_tilt: -3.6,
			treble_tilt: 3.6,
			coverage: { analyzed: 188, total_listened: 220 },
		},
	};

	const profile = $derived<AudioProfile>(variants[variant]);
</script>

{#if import.meta.env.DEV}
	<div class="preview">
		<header>
			<h1>Audio profile — preview</h1>
			<p class="subtitle">
				<code>AudioProfileStrip</code> — DR, loudness, bass tilt, treble tilt across
				the window's analysed listens. Dev-only route.
			</p>
		</header>

		<div class="controls">
			<label>
				<span class="control-label">Variant</span>
				<select bind:value={variant}>
					<option value="captured">Captured (real DB)</option>
					<option value="loud">Loud · low DR</option>
					<option value="dynamic">Dynamic · classical-leaning</option>
					<option value="bass-heavy">Bass heavy</option>
					<option value="treble-heavy">Treble heavy</option>
				</select>
			</label>
		</div>

		<AudioProfileStrip {profile} />

		<aside class="info">
			<h2>Lineage</h2>
			<dl>
				<dt>Dynamic range</dt>
				<dd>P95 − P5 of <code>audio_dsp_features.loudness_lufs</code>, listen-weighted.</dd>
				<dt>Loudness</dt>
				<dd>Listen-weighted mean of <code>audio_dsp_features.loudness_lufs</code>.</dd>
				<dt>Bass / Treble tilt</dt>
				<dd>
					<code>clamp(20 · log10(2000 / mean_centroid), -6, +6)</code> from
					<code>audio_dsp_features.spectral_centroid</code>; treble = −bass.
				</dd>
				<dt>Coverage</dt>
				<dd>Analysed / total-listened tracks in window.</dd>
			</dl>
			<p class="hint">
				Tilt is heuristic, not measured EQ — caption inside the card states this.
				Page hides the strip entirely when <code>coverage.analyzed === 0</code>.
				Missing values render <code>--</code> per <code>format.ts</code>.
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
		max-width: 1280px;
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

	.controls select {
		font-family: var(--font-mono);
		font-size: 0.78rem;
		background: var(--input-bg);
		border: 1px solid var(--input-border);
		color: var(--text-primary);
		padding: 4px 8px;
		border-radius: var(--radius-xs);
	}

	.control-label {
		font-family: var(--font-mono);
		font-size: 0.65rem;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--text-tertiary);
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
		font-family: var(--font-body);
		font-size: 0.85rem;
	}

	.info dt {
		font-family: var(--font-mono);
		font-size: 0.78rem;
		color: var(--text-tertiary);
	}

	.info dd {
		margin: 0;
		color: var(--text-primary);
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
