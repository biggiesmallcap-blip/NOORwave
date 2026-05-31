<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { api, authFetch, getApiBase, getStoredToken, type AudioQuality } from '$lib/api/client';
	import { markLocalOnboardingComplete } from '$lib/onboarding/status';
	import TidalConnect from '$lib/components/onboarding/TidalConnect.svelte';
	import ListeningServicesConnect from '$lib/components/onboarding/ListeningServicesConnect.svelte';
	let step = $state(0);
	let tidalConnected = $state(false);
	let syncErrorMessage = $state('');
	let completing = $state(false);
	let completeError = $state('');
	let audioChoice = $state<'bit-perfect' | 'standard' | 'later' | null>(null);
	let audioApplyError = $state('');
	const isWindows = typeof navigator !== 'undefined' && /Win/i.test(navigator.platform);

	onMount(async () => {
		// `?preview` bypasses the auto-redirect so the page can be designed/QA'd
		// without unsetting onboarding state on the backend.
		const params = new URLSearchParams(window.location.search);
		if (params.has('preview')) return;
		try {
			const resp = await fetch(`${getApiBase()}/api/setup/onboarding`);
			if (resp.ok) {
				const { complete } = await resp.json();
				if (complete) {
					await goto('/', { replaceState: true });
				}
			}
		} catch {
			// Fail open — let the user proceed with onboarding rather than trap them.
		}
	});

	async function markComplete(): Promise<boolean> {
		try {
			const resp = await authFetch(`${getApiBase()}/api/setup/onboarding/complete`, {
				method: 'POST',
			});
			if (resp.ok) markLocalOnboardingComplete(getStoredToken());
			return resp.ok;
		} catch {
			return false;
		}
	}

	async function skipAll() {
		completing = true;
		await markComplete();
		completing = false;
		await goto('/', { replaceState: true });
	}

	function startBackgroundSync() {
		void authFetch(`${getApiBase()}/api/tidal/sync`, { method: 'POST' })
			.then(async (resp) => {
				if (!resp.ok) {
					const data = await resp.json().catch(() => ({}));
					throw new Error(data.message ?? `Sync returned ${resp.status}`);
				}
			})
			.catch((err) => {
				console.warn('background TIDAL sync failed', err);
				syncErrorMessage = "Library sync had a hiccup — you can retry from Settings.";
			});
	}

	function handleTidalConnected() {
		tidalConnected = true;
		startBackgroundSync();
		step = 2;
	}

	function handleListeningServicesDone() {
		step = 3;
	}

	async function applyAudioChoice(choice: 'bit-perfect' | 'standard' | 'later') {
		audioChoice = choice;
		audioApplyError = '';
		if (choice === 'later') {
			step = 4;
			return;
		}
		try {
			const current = await api.getAudioSettings();
			const next = {
				...current,
				quality: (choice === 'bit-perfect' ? 'HI_RES_LOSSLESS' : 'LOSSLESS') as AudioQuality,
				exclusive_mode: choice === 'bit-perfect' && isWindows,
				sample_rate_follow: choice === 'bit-perfect',
			};
			await api.updateAudioSettings(next);
			step = 4;
		} catch (err) {
			audioApplyError =
				err instanceof Error
					? err.message
					: "Couldn't save audio settings — you can set them later in Settings.";
		}
	}

	// Crossfade is handled at the layout level by the View Transitions API
	// (see routes/+layout.svelte onNavigate hook). No local dissolve animation
	// — that conflicted with the layout-level transition and read as a flash.
	async function finish() {
		completing = true;
		completeError = '';
		const ok = await markComplete();
		completing = false;
		if (!ok) {
			completeError = "Couldn't save your setup.";
			return;
		}
		await goto('/', { replaceState: true });
	}

	async function continueAnyway() {
		await goto('/', { replaceState: true });
	}
</script>

<svelte:head>
	<title>Welcome — NOORwave</title>
</svelte:head>

<div class="onboarding">
	<div class="stack">
		<img class="wordmark" src="/noor-logo-centered.svg" alt="NOORwave" />
		<div class="card">
		<div class="progress" role="tablist" aria-label="Onboarding steps">
			{#each Array(5) as _, i}
				<button
					type="button"
					class="dot"
					class:active={i <= step}
					class:current={i === step}
					disabled={i >= step}
					aria-label="Go back to step {i + 1}"
					aria-current={i === step ? 'step' : undefined}
					onclick={() => { if (i < step) step = i; }}
				></button>
			{/each}
		</div>

		{#if step === 0}
			<div class="step welcome">
				<h1>Welcome.</h1>
				<p class="lede">Pure sound. Perfect flow.</p>
				<button class="btn btn-primary" onclick={() => (step = 1)}>Get started</button>
				<button class="link" onclick={skipAll} disabled={completing}>Skip for now — set up later in Settings</button>
			</div>
		{:else if step === 1}
			<div class="step">
				<TidalConnect
					variant="onboarding"
					showSkip={true}
					onconnected={handleTidalConnected}
					onskip={() => (step = 2)}
				/>
			</div>
		{:else if step === 2}
			<div class="step">
				<ListeningServicesConnect
					oncontinue={handleListeningServicesDone}
					onskip={handleListeningServicesDone}
				/>
				{#if tidalConnected && !syncErrorMessage}
					<p class="footnote">Library syncing in the background…</p>
				{:else if syncErrorMessage}
					<p class="footnote warn">{syncErrorMessage}</p>
				{/if}
			</div>
		{:else if step === 3}
			<div class="step audio-quality">
				<h2>How should we play it?</h2>
				<p class="lede">Choose your output. You can change this anytime in Settings.</p>
				<div class="audio-choices">
					<button
						type="button"
						class="audio-choice"
						class:selected={audioChoice === 'bit-perfect'}
						onclick={() => void applyAudioChoice('bit-perfect')}
					>
						<span class="audio-choice-title">Bit-perfect <span class="audio-choice-pill">recommended</span></span>
						<span class="audio-choice-body">Hi-Res Lossless from Tidal{isWindows ? ', exclusive WASAPI grab,' : ''} and the device follows each track's native rate.</span>
					</button>
					<button
						type="button"
						class="audio-choice"
						class:selected={audioChoice === 'standard'}
						onclick={() => void applyAudioChoice('standard')}
					>
						<span class="audio-choice-title">Standard</span>
						<span class="audio-choice-body">Lossless CD-quality FLAC, shared output. Always works, easier to mix with other apps.</span>
					</button>
					<button
						type="button"
						class="audio-choice subtle"
						class:selected={audioChoice === 'later'}
						onclick={() => void applyAudioChoice('later')}
					>
						<span class="audio-choice-title">Decide later</span>
						<span class="audio-choice-body">Keep current defaults. You can change this in Settings → Audio anytime.</span>
					</button>
				</div>
				{#if audioApplyError}
					<p class="error" role="alert">{audioApplyError}</p>
				{/if}
			</div>
		{:else if step === 4}
			<div class="step done">
				<h2>You're all set</h2>
				<p class="lede">
					{tidalConnected
						? 'Your TIDAL library is syncing in the background. Tracks will appear as it finishes.'
						: 'You can connect TIDAL or Last.fm anytime from Settings.'}
				</p>
				{#if syncErrorMessage}
					<p class="warn">{syncErrorMessage}</p>
				{/if}
				{#if completeError}
					<p class="error" role="alert">{completeError}</p>
					<div class="actions">
						<button class="btn btn-primary" onclick={finish} disabled={completing}>Try again</button>
						<button class="link" onclick={continueAnyway}>Continue anyway</button>
					</div>
				{:else}
					<button class="btn btn-primary" onclick={finish} disabled={completing}>
						{completing ? 'Saving…' : 'Open NOORwave'}
					</button>
				{/if}
			</div>
		{/if}
	</div>
	</div>
</div>

<style>
	:global(html, body) {
		margin: 0;
		height: 100%;
	}
	.onboarding {
		position: fixed;
		inset: 0;
		display: grid;
		place-items: center;
		/* Transparent — the layout-level wallpaper-layer provides the
		   standing-wave shader (forced during the /onboarding route in
		   +layout.svelte). Persisting the same WebGL canvas across the
		   navigation to home prevents the shader-remount flash. */
		background: transparent;
		color: var(--text-primary);
		font-family: var(--font-body);
		padding: 32px;
		overflow: auto;
		scrollbar-gutter: stable both-edges;
	}
	.card {
		position: relative;
		z-index: 1;
		width: 100%;
		max-width: 520px;
		background: rgba(10, 12, 18, 0.62);
		border: 1px solid var(--border-subtle);
		border-radius: 20px;
		padding: 40px 36px 32px;
		display: flex;
		flex-direction: column;
		gap: 24px;
		backdrop-filter: var(--blur-overlay);
		-webkit-backdrop-filter: var(--blur-overlay);
	}
	.progress {
		display: flex;
		gap: 8px;
		justify-content: center;
	}
	.dot {
		width: 24px;
		height: 4px;
		border-radius: 2px;
		background: rgba(255, 255, 255, 0.08);
		border: none;
		padding: 0;
		cursor: pointer;
		transition: background 200ms, transform 120ms;
	}
	.dot:disabled { cursor: default; }
	.dot:not(:disabled):hover { background: rgba(255, 255, 255, 0.55); transform: scaleY(1.4); }
	.dot.active { background: rgba(255, 255, 255, 0.4); }
	.dot.current { background: #fff; }
	.step {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 16px;
		min-height: 240px;
		justify-content: center;
		text-align: center;
	}
	.stack {
		position: relative;
		width: 100%;
		max-width: 520px;
		display: flex;
		justify-content: center;
	}
	.stack .wordmark {
		position: absolute;
		bottom: calc(100% + 16px);
		left: 50%;
		transform: translateX(-50%);
		width: clamp(220px, 28vw, 320px);
		height: auto;
		filter: drop-shadow(0 12px 32px rgba(45, 212, 212, 0.18));
	}
	.step h1, .step h2 {
		margin: 0;
		font-family: var(--font-display);
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-medium);
		letter-spacing: -0.02em;
		line-height: var(--line-height-tight);
	}
	.lede {
		margin: 0;
		max-width: 420px;
		color: var(--text-secondary);
		line-height: var(--line-height-loose);
		font-size: var(--font-size-md);
	}
	.btn {
		font: inherit;
		padding: 12px 28px;
		border-radius: 10px;
		border: 1px solid transparent;
		cursor: pointer;
		font-weight: var(--font-weight-medium);
		transition: background 120ms;
	}
	.btn-primary { background: rgba(255, 255, 255, 0.92); color: #0a0d14; }
	.btn-primary:hover:not(:disabled) { background: #fff; }
	.btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }
	.link {
		background: none;
		border: none;
		color: var(--text-tertiary);
		font: inherit;
		font-size: var(--font-size-xs);
		cursor: pointer;
		padding: 4px 8px;
		text-decoration: underline;
		text-decoration-color: color-mix(in srgb, var(--text-tertiary) 40%, transparent);
		text-underline-offset: 3px;
		transition: color var(--motion-fast);
	}
	.link:hover { color: var(--text-primary); }
	.link:disabled { opacity: 0.5; cursor: not-allowed; }
	.actions {
		display: flex;
		gap: 12px;
		align-items: center;
		justify-content: center;
		flex-wrap: wrap;
	}
	.footnote {
		margin: 0;
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
	}
	.warn { color: var(--state-warning); }
	.error { color: var(--state-error); margin: 0; }
	.audio-quality {
		gap: 12px;
	}
	.audio-choices {
		display: flex;
		flex-direction: column;
		gap: 8px;
		width: 100%;
		max-width: 420px;
	}
	.audio-choice {
		text-align: left;
		background: var(--panel-bg);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-md);
		padding: 10px 14px;
		cursor: pointer;
		color: inherit;
		font: inherit;
		display: flex;
		flex-direction: column;
		gap: 2px;
		transition: background var(--motion-fast), border-color var(--motion-fast);
	}
	.audio-choice:hover {
		background: var(--bg-hover);
		border-color: var(--border-strong);
	}
	.audio-choice.selected {
		border-color: rgba(255, 255, 255, 0.6);
		background: rgba(255, 255, 255, 0.08);
	}
	.audio-choice.subtle {
		background: transparent;
	}
	.audio-choice-title {
		font-weight: var(--font-weight-semibold);
		font-size: var(--font-size-sm);
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.audio-choice-pill {
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-medium);
		padding: 2px 6px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.16);
		color: rgba(255, 255, 255, 0.92);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.audio-choice-body {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		line-height: var(--line-height-normal);
	}
</style>
