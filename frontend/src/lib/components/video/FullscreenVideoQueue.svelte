<!--
	Up-next panel inside the fullscreen player (Queue button or Q). Same rows
	and menu as the shell's video queue, plus inline Move next and Remove.
-->
<script lang="ts">
	import { onMount } from 'svelte';
	import VideoQueueList from './VideoQueueList.svelte';
	import { videoChannelOnAir, videoSession, videoSessionUpcoming } from '$lib/stores/video_session';

	let { onClose }: { onClose: () => void } = $props();

	let panel: HTMLElement | null = $state(null);

	// Keyboard users land in the panel, so Tab walks the queue.
	onMount(() => panel?.focus({ preventScroll: true }));

	function keepFocus(event: FocusEvent) {
		if (event.relatedTarget) return;
		// Playing, moving or removing a row re-renders it and drops focus to
		// the page; keep it in the panel so Tab continues from here.
		requestAnimationFrame(() => {
			if (panel?.isConnected && document.activeElement === document.body) panel.focus({ preventScroll: true });
		});
	}
</script>

<section
	bind:this={panel}
	id="fullscreen-video-queue"
	class="fullscreen-queue"
	aria-label="Video queue"
	tabindex="-1"
	onfocusout={keepFocus}
>
	<header>
		<h2 class="t-section">Queue</h2>
		<span class="t-meta">
			{#if $videoChannelOnAir}<span class="channel">{String($videoChannelOnAir.no).padStart(2, '0')} {$videoChannelOnAir.title}</span>{/if}
			{$videoSessionUpcoming.length} up next
		</span>
		<button type="button" class="close" onclick={onClose} aria-label="Close video queue" title="Close queue (Q)">&#x2715;</button>
	</header>
	<div class="queue-settings">
		<button
			type="button"
			aria-pressed={$videoSession.continuous}
			onclick={() => $videoSession.continuous ? videoSession.stopRadio() : videoSession.startRadio()}
		>
			<!-- On a channel, stopping its refills is leaving the channel. -->
			{$videoChannelOnAir ? 'Leave channel' : $videoSession.continuous ? 'Stop radio' : 'Start radio'}
		</button>
		<button
			type="button"
			aria-pressed={$videoSession.autoplay}
			onclick={() => videoSession.setAutoplay(!$videoSession.autoplay)}
		>
			Autoplay {$videoSession.autoplay ? 'on' : 'off'}
		</button>
	</div>
	{#if $videoSessionUpcoming.length > 0}
		<VideoQueueList videos={$videoSessionUpcoming} actions />
		{#if $videoSession.radioSearching}<p class="t-meta" role="status">Finding more videos...</p>{/if}
	{:else}
		<p class="t-body">{$videoSession.continuous ? 'Finding more videos...' : 'No videos up next. Start radio to add more.'}</p>
	{/if}
	{#if $videoSession.radioIssue}<p class="t-meta" role="status">{$videoSession.radioIssue}</p>{/if}
</section>

<style>
	/* The overlay recipe minus --blur-modal: at 96 percent the blur is
	   invisible, and re-blurring live video every frame costs real GPU. */
	.fullscreen-queue {
		position: absolute;
		z-index: 4;
		top: var(--space-4);
		right: var(--space-4);
		bottom: calc(100px + var(--space-4));
		display: flex;
		flex-direction: column;
		gap: var(--gap-sm);
		width: min(460px, calc(100% - 2 * var(--space-4)));
		padding: var(--space-4);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		background: color-mix(in srgb, var(--bg-elevated) 96%, transparent);
		color: var(--text-primary);
		box-shadow: var(--panel-shadow);
		outline: none;
	}

	header,
	.queue-settings {
		display: flex;
		align-items: center;
		gap: var(--gap-sm);
	}

	header h2,
	p {
		margin: 0;
	}

	button {
		color: var(--text-primary);
		cursor: pointer;
	}

	button:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.channel {
		margin-right: var(--space-2);
		font-family: var(--font-mono);
		color: var(--text-secondary);
	}

	.close {
		flex-shrink: 0;
		width: 36px;
		height: 36px;
		margin-left: auto;
		border-radius: 50%;
	}

	.close:hover {
		background: var(--bg-hover);
	}

	.queue-settings button {
		min-height: 36px;
		padding: 0 var(--space-3);
		border-radius: 999px;
		background: var(--bg-surface);
	}

	.queue-settings button[aria-pressed='true'] {
		color: var(--accent-strong);
		background: var(--accent-soft);
	}
</style>
