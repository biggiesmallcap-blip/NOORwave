<script lang="ts">
	import { onDestroy } from 'svelte';
	import { fade } from 'svelte/transition';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { lowerVideoCurtain, videoCurtain } from '$lib/video/curtain';

	// The curtain between videos (lib/video/curtain.ts), drawn over the
	// picture and under everything else on the stage. It lifts once the new
	// video has arrived and its position moves forward, on the new video's
	// own failure, or after CURTAIN_MAX_MS whatever the stream does.
	let { currentId, positionS, startedAtS, failed }: {
		currentId: number | null;
		positionS: number;
		/** Where the new video starts for us (a tune-in lands mid-video). */
		startedAtS: number;
		/** The new video could not play: lift and let the player say so. */
		failed: boolean;
	} = $props();

	const CURTAIN_MAX_MS = 8000;
	/** Played this far into the new video: it is really on. */
	const CURTAIN_LIFT_S = 0.25;

	let timer: ReturnType<typeof setTimeout> | null = null;
	/** The position last seen since the new video arrived: only a position
	 *  that moved forward after that lifts it, so the old video's position
	 *  (still reported for a moment) cannot. */
	let seen: number | null = null;
	let raisedSeq = 0;

	// A new raise: start its timer and forget positions seen before it.
	$effect(() => {
		const curtain = $videoCurtain;
		if (!curtain || curtain.seq === raisedSeq) return;
		raisedSeq = curtain.seq;
		seen = null;
		if (timer) clearTimeout(timer);
		timer = setTimeout(lowerVideoCurtain, CURTAIN_MAX_MS);
	});

	$effect(() => {
		const curtain = $videoCurtain;
		if (!curtain) return;
		const arrived = currentId !== null && currentId !== curtain.fromId;
		if (!arrived) return;
		const last = seen;
		seen = positionS;
		const moving = last !== null && positionS > last && positionS >= startedAtS + CURTAIN_LIFT_S;
		if (moving || failed) lowerVideoCurtain();
	});

	onDestroy(() => {
		if (timer) clearTimeout(timer);
		lowerVideoCurtain();
	});
</script>

{#if $videoCurtain}
	<div class="curtain" out:fade={{ duration: 420 }} aria-hidden="true">
		{#if $videoCurtain.art}
			<ArtworkImage src={$videoCurtain.art} size={640} fallbackText="" decorative fadeIn loading="eager" fetchPriority="high" />
		{/if}
	</div>
{/if}

<style>
	/* Its own still, full bleed, softened and a little dark, drifting in
	   slowly. Black underneath, never a colour. On the player's overlay
	   layer (after it, so over its loading text and bar); the fullscreen
	   pills and the channel's bug and credits stay above. */
	.curtain {
		position: absolute;
		inset: 0;
		z-index: 2;
		overflow: hidden;
		background: #000;
		pointer-events: none;
		animation: curtain-in var(--motion-fast) both;
	}
	.curtain :global(img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		filter: brightness(0.62) saturate(0.9) blur(1.5px);
		transform: scale(1.04);
		animation: curtain-drift 6s ease-out both; /* motion-ok: a slow drift under the still, not a UI transition */
	}
	/* Film grain over the still, so it reads as picture, not a frozen
	   thumbnail: a seamless tile blended softly, jittering a few times a
	   second like a projector. */
	.curtain::after {
		content: '';
		position: absolute;
		inset: -32px;
		background: url('/textures/film-grain.webp') repeat;
		background-size: 627px 627px;
		mix-blend-mode: overlay;
		opacity: 0.6;
		pointer-events: none;
		animation: grain 0.9s steps(1) infinite;
	}
	@keyframes grain {
		0% { transform: translate(0, 0); }
		20% { transform: translate(-13px, 9px); }
		40% { transform: translate(11px, -17px); }
		60% { transform: translate(-7px, 21px); }
		80% { transform: translate(19px, -5px); }
	}
	@keyframes curtain-in {
		from { opacity: 0; }
		to { opacity: 1; }
	}
	@keyframes curtain-drift {
		from { transform: scale(1.08); }
		to { transform: scale(1.04); }
	}
	@media (prefers-reduced-motion: reduce) {
		.curtain::after { animation: none; }
	}
	:global(:root[data-motion='reduce']) .curtain::after { animation: none; }
</style>
