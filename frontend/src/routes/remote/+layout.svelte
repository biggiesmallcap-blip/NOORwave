<script lang="ts">
	import RemoteActionSheet from '$lib/components/remote/RemoteActionSheet.svelte';
	import { installMediaSessionBridge } from '$lib/remote/mediaSession';
	import { installSilentMediaLoop } from '$lib/remote/silentLoop';
	import { installWakeLock } from '$lib/remote/wakeLock';
	import { audioSettings } from '$lib/stores/audio_settings';
	import { wsConnected } from '$lib/api/ws';
	import { currentTrack, playerError, refreshPlaybackRuntime } from '$lib/stores/player';
	import { showToast } from '$lib/stores/toast';
	import { upscaleTidalArtwork } from '$lib/utils/artwork';
	import type { Snippet } from 'svelte';

	// The whole /remote tree shares one mount of:
	//   - the long-press action sheet,
	//   - the silent <audio> loop (iOS lockscreen anchor),
	//   - MediaSession + WakeLock bridges,
	//   - the blurred backdrop and disconnect banner.
	// Previously these were duplicated on the home page and re-installed on
	// every navigation back, which caused a 3-5s teardown/setup stall. Hosting
	// them on the layout keeps them mounted across sub-page swaps.
	let { children }: { children: Snippet } = $props();

	$effect(() => {
		const err = $playerError;
		if (err) showToast(err.message, 'error');
	});

	let silentLoopEl: HTMLAudioElement | null = $state(null);
	$effect(() => {
		if (!silentLoopEl) return;
		const teardownMediaSession = installMediaSessionBridge();
		const teardownWakeLock = installWakeLock();
		const teardownSilentLoop = installSilentMediaLoop(silentLoopEl);
		return () => {
			teardownMediaSession();
			teardownWakeLock();
			teardownSilentLoop();
		};
	});

	$effect(() => {
		void refreshPlaybackRuntime();
		void audioSettings.load();
	});

	let backdropArt = $derived(upscaleTidalArtwork($currentTrack?.artwork_url));

	const DISCONNECT_GRACE_MS = 4000;
	let hasEverConnected = $state(false);
	let showDisconnected = $state(false);
	$effect(() => {
		if ($wsConnected) {
			hasEverConnected = true;
			showDisconnected = false;
			return;
		}
		if (!hasEverConnected) return;
		const id = setTimeout(() => {
			showDisconnected = true;
		}, DISCONNECT_GRACE_MS);
		return () => clearTimeout(id);
	});
</script>

<div class="remote-layout-backdrop" aria-hidden="true">
	{#if backdropArt}
		<img src={backdropArt} alt="" />
	{/if}
</div>

<audio
	bind:this={silentLoopEl}
	src="/silent.wav"
	loop
	playsinline
	preload="none"
	aria-hidden="true"
></audio>

{#if showDisconnected}
	<div class="remote-layout-banner" role="status" aria-live="polite">
		<span class="remote-layout-banner-dot" aria-hidden="true"></span>
		Reconnecting to the server…
	</div>
{/if}

{@render children()}

<RemoteActionSheet />

<style>
	.remote-layout-backdrop {
		position: fixed;
		inset: 0;
		z-index: 0;
		overflow: hidden;
		background: var(--surface-0);
		pointer-events: none;
	}

	.remote-layout-backdrop img {
		position: absolute;
		inset: -10%;
		width: 120%;
		height: 120%;
		object-fit: cover;
		opacity: 0.35;
		filter: blur(60px) saturate(160%);
		transform: scale(1.1);
		/* Pin to its own compositing layer so the expensive blur+saturate
		   filter is rasterized once and reused across page swaps instead of
		   re-running on every navigation paint. */
		will-change: transform;
	}

	.remote-layout-banner {
		position: sticky;
		top: 0;
		z-index: 40;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 8px;
		padding: 8px 14px calc(8px + env(safe-area-inset-top));
		background: color-mix(in oklab, var(--state-error) 22%, var(--bg-base));
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.02em;
	}

	.remote-layout-banner-dot {
		width: 8px;
		height: 8px;
		border-radius: 999px;
		background: var(--state-error);
		box-shadow: 0 0 0 0 var(--state-error);
		animation: remote-layout-pulse 1400ms ease-in-out infinite;
	}

	@keyframes remote-layout-pulse {
		0%,
		100% {
			box-shadow: 0 0 0 0 color-mix(in oklab, var(--state-error) 60%, transparent);
		}
		50% {
			box-shadow: 0 0 0 6px color-mix(in oklab, var(--state-error) 0%, transparent);
		}
	}
</style>
