<script lang="ts">
	/**
	 * AudioProfileStrip — four-up summary of the window's audio character.
	 *
	 *   DYNAMIC RANGE   9.1 DR
	 *   LOUDNESS       -11.2 LUFS
	 *   BASS TILT       +2.3   (no dB unit — derived from spectral centroid)
	 *   TREBLE TILT     -1.1
	 *
	 * Hidden by the page when audio_profile.coverage.analyzed === 0.
	 *
	 * Spec: C:\Users\Felix\.claude\plans\lets-revision-analytics-stats-crystalline-melody.md
	 */

	import type { AudioProfile } from '$lib/api/client';
	import { formatCount, formatDr, formatLufs, formatTilt } from '$lib/utils/format';

	interface Props {
		profile: AudioProfile;
	}

	let { profile }: Props = $props();

	const trackCoverage = $derived(profile.track_coverage ?? profile.coverage);
</script>

<section class="audio glass" aria-label="Audio profile">
	<header class="head">
		<span class="eyebrow">Audio profile</span>
		<span class="coverage">
			{formatCount(trackCoverage.analyzed)} / {formatCount(trackCoverage.total_listened)} analysed
		</span>
	</header>

	<div class="cells">
		<div class="cell">
			<span class="label">Dynamic range</span>
			<span class="value">{formatDr(profile.dynamic_range_dr)}</span>
		</div>
		<div class="cell">
			<span class="label">Loudness</span>
			<span class="value">{formatLufs(profile.loudness_lufs)}</span>
		</div>
		<div class="cell">
			<span class="label">Bass tilt</span>
			<span class="value">{formatTilt(profile.bass_tilt)}</span>
		</div>
		<div class="cell">
			<span class="label">Treble tilt</span>
			<span class="value">{formatTilt(profile.treble_tilt)}</span>
		</div>
	</div>

	<p class="note">Tilt is derived from spectral centroid, not measured EQ.</p>
</section>

<style>
	.audio {
		padding: var(--space-4);
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--space-3);
	}

	.eyebrow {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-tertiary);
	}

	.coverage {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
		font-variant-numeric: tabular-nums;
	}

	.cells {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: 1px;
		background: var(--border-subtle);
		border-radius: var(--radius-sm);
		overflow: hidden;
	}

	.cell {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		padding: var(--space-3) var(--space-4);
		background: linear-gradient(
				180deg,
				color-mix(in srgb, var(--instrument-surface) 60%, transparent),
				color-mix(in srgb, var(--instrument-surface-strong) 76%, transparent)
			),
			var(--panel-bg);
	}

	.label {
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-tertiary);
	}

	.value {
		font-family: var(--font-display);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-medium);
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
		letter-spacing: -0.01em;
	}

	.note {
		margin: 0;
		font-family: var(--font-body);
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		opacity: 0.85;
	}

	@media (max-width: 720px) {
		.cells {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}
</style>
