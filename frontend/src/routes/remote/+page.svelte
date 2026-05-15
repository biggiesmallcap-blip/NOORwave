<script lang="ts">
	import RemoteMiniSearch from '$lib/components/remote/RemoteMiniSearch.svelte';
	import RemoteQueue from '$lib/components/remote/RemoteQueue.svelte';
	import RemoteSettingsPill from '$lib/components/remote/RemoteSettingsPill.svelte';
	import RemoteTransport from '$lib/components/remote/RemoteTransport.svelte';
	import { goto } from '$app/navigation';
	import { hapticTap } from '$lib/remote/haptics';
	import {
		currentTrack,
		isPlaying,
		playbackQueue,
		playerReady,
		position,
		volume
	} from '$lib/stores/player';

	// Backdrop, MediaSession/WakeLock/silentLoop bridges, action sheet mount,
	// disconnect banner, and refreshPlaybackRuntime/audioSettings.load live in
	// /remote/+layout.svelte so they persist across sub-page navigations.
</script>

<svelte:head>
	<title>NOOR Remote</title>
</svelte:head>

<main class="remote-page" aria-label="NOOR remote">
	<header class="remote-header">
		<div class="remote-header-text">
			<p>NOOR Remote</p>
			<span>{$playerReady ? 'Connected' : 'Connecting'}</span>
		</div>
		<div class="remote-header-actions">
			<button
				type="button"
				class="remote-library-pill"
				aria-label="Open library"
				onclick={() => {
					hapticTap();
					void goto('/remote/library');
				}}
			>
				<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
					<path
						d="M4 5h4v14H4zM10 5h4v14h-4zM17 5l3 1-3 13-3-1z"
						fill="none"
						stroke="currentColor"
						stroke-width="1.6"
						stroke-linejoin="round"
					/>
				</svg>
				<span>Library</span>
			</button>
			<RemoteSettingsPill />
		</div>
	</header>

	<RemoteTransport track={$currentTrack} isPlaying={$isPlaying} position={$position} volume={$volume} />

	<RemoteMiniSearch />

	<RemoteQueue queue={$playbackQueue} currentTrack={$currentTrack} />
</main>

<style>
	.remote-page {
		position: relative;
		z-index: 1;
		min-height: 100svh;
		padding: max(18px, env(safe-area-inset-top)) 16px max(22px, env(safe-area-inset-bottom));
		color: var(--text-primary);
		display: grid;
		gap: 20px;
		align-content: start;
	}

	.remote-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
	}

	.remote-header-text {
		display: flex;
		align-items: baseline;
		gap: 10px;
		min-width: 0;
	}

	.remote-header-actions {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-shrink: 0;
	}

	.remote-library-pill {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		min-height: 32px;
		padding: 0 12px;
		border-radius: 999px;
		background: var(--surface-1);
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.remote-library-pill svg {
		width: 14px;
		height: 14px;
	}

	.remote-library-pill:active {
		background: var(--surface-2);
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
