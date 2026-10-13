<script lang="ts">
	// Marks a page whose content comes from Spotify (external, matched to
	// TIDAL for playback), so the source reads at a glance.
	let { label = 'Spotify' }: { label?: string } = $props();
</script>

<span class="spotify-badge">
	<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true">
		<circle cx="12" cy="12" r="11" fill="currentColor" />
		<path d="M6.5 9.2c3.8-1.1 8-.8 11.2 1M7.1 12.3c3.1-.9 6.5-.6 9.2.9M7.7 15.2c2.5-.7 5.1-.5 7.2.7"
			fill="none" stroke="var(--spotify-badge-ink)" stroke-width="1.6" stroke-linecap="round" />
	</svg>
	{label}
</span>

<style>
	.spotify-badge {
		--spotify-badge-ink: #0b1f13;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 3px 10px 3px 4px;
		border-radius: 999px;
		background: rgba(30, 215, 96, 0.14);
		color: #1ed760;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		line-height: 1.2;
		white-space: nowrap;
	}
</style>
