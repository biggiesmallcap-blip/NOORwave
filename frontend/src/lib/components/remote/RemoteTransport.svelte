<script lang="ts">
	import type { Track } from '$lib/api/client';
	import {
		playNextTrack,
		playPreviousTrack,
		setPlayerPosition,
		setPlayerVolume,
		togglePlayback
	} from '$lib/stores/player';
	import { formatTrackDuration } from '$lib/utils/format';

	let {
		track,
		isPlaying,
		position,
		volume
	}: {
		track: Track | null;
		isPlaying: boolean;
		position: number;
		volume: number;
	} = $props();

	let localPosition = $state(position);
	let localVolume = $state(Math.round(volume * 100));

	$effect(() => {
		localPosition = position;
	});

	$effect(() => {
		localVolume = Math.round(volume * 100);
	});

	function commitSeek() {
		void setPlayerPosition(localPosition);
	}

	function commitVolume() {
		void setPlayerVolume(localVolume / 100);
	}
</script>

<section class="remote-transport" aria-label="Playback controls">
	<div class="remote-art">
		{#if track?.artwork_url}
			<img src={track.artwork_url} alt="" />
		{:else}
			<div class="remote-art-empty" aria-hidden="true">NOOR</div>
		{/if}
	</div>

	<div class="remote-copy">
		<strong>{track?.title ?? 'Nothing playing'}</strong>
		<span>{track?.artist_name ?? 'Choose a track to begin playback.'}</span>
	</div>

	<div class="remote-seek">
		<input
			type="range"
			min="0"
			max={track?.duration_ms ?? 0}
			step="1000"
			bind:value={localPosition}
			onchange={commitSeek}
			disabled={!track?.duration_ms}
			aria-label="Seek playback"
		/>
		<div class="remote-time">
			<span>{formatTrackDuration(localPosition)}</span>
			<span>{formatTrackDuration(track?.duration_ms ?? 0)}</span>
		</div>
	</div>

	<div class="remote-buttons" role="group" aria-label="Transport">
		<button type="button" aria-label="Previous" onclick={() => void playPreviousTrack()}>Prev</button>
		<button class="primary" type="button" aria-label="Play or pause" onclick={() => void togglePlayback()}>
			{isPlaying ? 'Pause' : 'Play'}
		</button>
		<button type="button" aria-label="Next" onclick={() => void playNextTrack()}>Next</button>
	</div>

	<label class="remote-volume">
		<span>Volume</span>
		<input type="range" min="0" max="100" step="1" bind:value={localVolume} onchange={commitVolume} aria-label="Volume" />
	</label>
</section>

<style>
	.remote-transport {
		display: grid;
		gap: 16px;
	}

	.remote-art {
		aspect-ratio: 1;
		border-radius: 8px;
		overflow: hidden;
		background: var(--surface-1);
	}

	.remote-art img,
	.remote-art-empty {
		width: 100%;
		height: 100%;
	}

	.remote-art img {
		object-fit: cover;
	}

	.remote-art-empty {
		display: grid;
		place-items: center;
		color: var(--text-muted);
	}

	.remote-copy,
	.remote-seek,
	.remote-volume {
		display: grid;
		gap: 8px;
	}

	.remote-copy strong,
	.remote-copy span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-time,
	.remote-buttons {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
	}

	.remote-buttons button {
		min-height: 48px;
		flex: 1;
	}

	.remote-buttons .primary {
		flex: 1.4;
	}
</style>
