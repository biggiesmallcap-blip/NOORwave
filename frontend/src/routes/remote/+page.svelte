<script lang="ts">
	import { currentTrack, isPlaying, playbackQueue, playerReady } from '$lib/stores/player';
</script>

<svelte:head>
	<title>NOOR Remote</title>
</svelte:head>

<main class="remote-page" aria-label="NOOR remote">
	<section class="remote-hero" aria-live="polite">
		<p class="remote-kicker">NOOR Remote</p>
		<h1>{$currentTrack?.title ?? ($playerReady ? 'Ready' : 'Connecting')}</h1>
		<p>{$currentTrack?.artist_name ?? ($isPlaying ? 'Playing' : 'Choose something to play')}</p>
	</section>

	<section class="remote-placeholder" aria-label="Remote status">
		<span>{$playbackQueue.length} queued</span>
	</section>
</main>

<style>
	.remote-page {
		min-height: 100svh;
		padding: max(18px, env(safe-area-inset-top)) 16px max(22px, env(safe-area-inset-bottom));
		background: var(--surface-0);
		color: var(--text-primary);
	}

	.remote-hero,
	.remote-placeholder {
		display: grid;
		gap: 8px;
	}

	.remote-kicker {
		margin: 0;
		color: var(--text-muted);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
	}

	.remote-hero h1,
	.remote-hero p,
	.remote-placeholder span {
		margin: 0;
	}
</style>
