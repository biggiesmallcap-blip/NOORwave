<script lang="ts">
	import type { VideoChannel } from '$lib/api/video_explore_types';
	import { channelNumber } from './clock';
	import { channelHue } from './lineup';
	import { pressRemote } from './remote';

	// The channel you are on, beside the heart in fullscreen: the arrows flip
	// down and up, the name opens the guide. The remote's keys, for a mouse.
	let { channel }: { channel: VideoChannel } = $props();
</script>

<div class="channel-pill" style:--ch-hue={channelHue(channel)}>
	<button type="button" class="flip" aria-label="Channel down" title="Channel down" onclick={() => pressRemote({ kind: 'flip', step: -1 })}>
		<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 9l6 6 6-6" /></svg>
	</button>
	<button type="button" class="name" title="Guide (G)" aria-label={`Channel ${channel.no}, ${channel.title}: open the guide`} onclick={() => pressRemote({ kind: 'guide' })}>
		<span class="no">{channelNumber(channel.no)}</span>{channel.title}
	</button>
	<button type="button" class="flip" aria-label="Channel up" title="Channel up" onclick={() => pressRemote({ kind: 'flip', step: 1 })}>
		<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 15l6-6 6 6" /></svg>
	</button>
</div>

<style>
	/* The same light wash as the heart and Queue pills beside it. */
	.channel-pill {
		display: flex;
		align-items: stretch;
		height: 34px;
		border: 1px solid rgba(255, 255, 255, 0.16);
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.1);
		overflow: hidden;
	}
	button {
		display: flex;
		align-items: center;
		border: 0;
		background: transparent;
		color: rgba(255, 255, 255, 0.84);
		font: inherit;
		cursor: pointer;
		transition:
			background var(--motion-fast),
			color var(--motion-fast);
	}
	button:hover {
		background: rgba(255, 255, 255, 0.12);
		color: #fff;
	}
	button:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: -2px;
	}
	.flip {
		justify-content: center;
		width: 30px;
		padding: 0;
	}
	.flip svg {
		width: 14px;
		height: 14px;
		fill: none;
		stroke: currentColor;
		stroke-width: 2.2;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	.name {
		gap: 8px;
		max-width: 240px;
		padding: 0 6px;
		font-size: var(--font-size-xs);
		font-weight: 800;
		white-space: nowrap;
	}
	.no {
		font-family: var(--font-mono);
		color: oklch(80% 0.13 calc(var(--ch-hue) * 1deg));
	}
</style>
