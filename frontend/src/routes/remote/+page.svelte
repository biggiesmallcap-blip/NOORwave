<script lang="ts">
	import RemoteQueue from '$lib/components/remote/RemoteQueue.svelte';
	import RemoteTransport from '$lib/components/remote/RemoteTransport.svelte';
	import {
		currentTrack,
		isPlaying,
		playbackQueue,
		playerReady,
		position,
		volume
	} from '$lib/stores/player';
</script>

<svelte:head>
	<title>NOOR Remote</title>
</svelte:head>

<main class="remote-page" aria-label="NOOR remote">
	<header class="remote-header">
		<p>NOOR Remote</p>
		<span>{$playerReady ? 'Connected' : 'Connecting'}</span>
	</header>

	<RemoteTransport track={$currentTrack} isPlaying={$isPlaying} position={$position} volume={$volume} />

	<RemoteQueue queue={$playbackQueue} />
</main>

<style>
	.remote-page {
		min-height: 100svh;
		padding: max(18px, env(safe-area-inset-top)) 16px max(22px, env(safe-area-inset-bottom));
		background: var(--surface-0);
		color: var(--text-primary);
		display: grid;
		gap: 20px;
		align-content: start;
	}

	.remote-header {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 10px;
	}

	.remote-header p {
		margin: 0;
		color: var(--text-muted);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
	}

	.remote-header span {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}
</style>
