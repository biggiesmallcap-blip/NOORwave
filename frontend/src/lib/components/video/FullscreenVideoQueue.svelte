<script lang="ts">
	import {
		playQueuedVideo, playVideoNext, removeVideoFromQueue,
		videoSession, videoSessionUpcoming,
	} from '$lib/stores/video_session';
	import { formatTrackDuration } from '$lib/utils/format';

	let { onClose }: { onClose: () => void } = $props();
</script>

<section id="fullscreen-video-queue" class="fullscreen-queue" aria-label="Video queue">
	<header>
		<h2 class="t-section-title">Queue</h2>
		<span class="t-meta">{$videoSessionUpcoming.length} up next</span>
		<button type="button" class="close" onclick={onClose} aria-label="Close video queue" title="Close queue (Q)">✕</button>
	</header>
	<div class="queue-settings">
		<button type="button" aria-pressed={$videoSession.continuous}
			onclick={() => $videoSession.continuous ? videoSession.stopRadio() : videoSession.startRadio()}>
			{$videoSession.continuous ? 'Stop radio' : 'Start radio'}
		</button>
		<button type="button" aria-pressed={$videoSession.autoplay}
			onclick={() => videoSession.setAutoplay(!$videoSession.autoplay)}>
			Autoplay {$videoSession.autoplay ? 'on' : 'off'}
		</button>
	</div>
	<div class="queue-list">
		{#each $videoSessionUpcoming as video, i (`${video.tidal_id}-${i}`)}
			<div class="queue-row">
				<button type="button" class="play" onclick={() => void playQueuedVideo(video.tidal_id)} aria-label={`Play ${video.title}`}>
					<span class="number t-micro">{i + 1}</span>
					<span class="copy"><strong class="t-row-title">{video.title}</strong><span class="t-meta">{video.artist_name ?? 'Unknown artist'}</span></span>
					<span class="t-micro">{formatTrackDuration(video.duration_ms ?? 0)}</span>
				</button>
				<button type="button" class="row-action" onclick={() => playVideoNext(video)} disabled={i === 0}
					aria-label={`Play ${video.title} next`} title="Play next">↑</button>
				<button type="button" class="row-action" onclick={() => removeVideoFromQueue(video.tidal_id)}
					aria-label={`Remove ${video.title} from queue`} title="Remove from queue">✕</button>
			</div>
		{:else}
			<p class="t-body">{$videoSession.continuous ? 'Finding more videos…' : 'No videos up next. Start radio to add more.'}</p>
		{/each}
	</div>
	{#if $videoSession.error || $videoSession.radioIssue || $videoSession.radioSearching}
		<p class="t-meta" role="status">{$videoSession.error ?? $videoSession.radioIssue ?? 'Finding more videos…'}</p>
	{/if}
</section>

<style>
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
		backdrop-filter: var(--blur-modal);
	}

	header, .queue-settings { display: flex; align-items: center; gap: var(--gap-sm); }
	header h2 { margin: 0; }
	.close { margin-left: auto; }
	.queue-list { min-height: 0; overflow-y: auto; }
	.queue-row { display: flex; align-items: center; border-radius: var(--radius-sm); }
	.queue-row:hover, .queue-row:focus-within { background: var(--bg-hover); }
	.play { display: grid; grid-template-columns: 24px minmax(0, 1fr) auto; align-items: center; gap: var(--gap-sm); flex: 1; min-width: 0; padding: var(--space-2); text-align: left; }
	.copy { display: flex; flex-direction: column; min-width: 0; }
	.copy strong, .copy span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	button { color: var(--text-primary); cursor: pointer; }
	.close, .row-action { flex-shrink: 0; width: 36px; height: 36px; border-radius: 50%; }
	.close:hover, .row-action:hover { background: var(--bg-hover); }
	button:focus-visible { outline: 2px solid var(--accent-strong); outline-offset: -2px; }
	button:disabled { opacity: 0.35; cursor: not-allowed; }
	.queue-settings button { min-height: 36px; padding: 0 var(--space-3); border-radius: 999px; background: var(--bg-surface); }
	.queue-settings button[aria-pressed='true'] { color: var(--accent-strong); background: var(--accent-soft); }
</style>
