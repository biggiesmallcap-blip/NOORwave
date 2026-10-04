<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { palette } from '$lib/stores/palette';
	import { surfaceMode } from '$lib/stores/surfaceMode';
	import { wallpaper, setWallpaper } from '$lib/stores/wallpaper';
	import { applyPaletteTheme } from '$lib/components/wallpaper/paletteTheme';
	import AppearanceFields from '$lib/components/settings/AppearanceFields.svelte';
	import AppearancePreview from '$lib/components/onboarding/AppearancePreview.svelte';
	import { draftAppearance, changeAppearanceDraft, recoverAppearanceDraft, rememberAppearanceDraft, clearAppearanceRecovery, persistAppearanceDraft } from '$lib/onboarding/appearanceDraft';
	import { syncStatus, syncProgress, syncError, startTidalSync } from '$lib/stores/tidal';
	import '$lib/components/settings/settings.css';
	import '$lib/components/onboarding/onboarding.css';
	const savedAppearance = { palette: get(palette), theme: get(surfaceMode), background: get(wallpaper) };
	let appearanceDraft = $state(draftAppearance(savedAppearance));
	let brandFrame = $state<HTMLDivElement>();
	$effect(() => { if (brandFrame) applyPaletteTheme(brandFrame, 'futuro', 'dark'); });
	import { goto } from '$app/navigation';
	import { api, authFetch, getApiBase, getStoredToken, type AudioQuality } from '$lib/api/client';
	import { markLocalOnboardingComplete } from '$lib/onboarding/status';
	import TidalConnect from '$lib/components/onboarding/TidalConnect.svelte';
	import ListeningServicesConnect from '$lib/components/onboarding/ListeningServicesConnect.svelte';
	let step = $state(0);
	let tidalConnected = $state(false);
	let syncRequestError = $state('');
	let syncRequested = $state(false);
	let syncRequestPending = $state(false);
	let syncErrorMessage = $derived($syncError ?? syncRequestError);
	let completing = $state(false);
	let completeError = $state('');
	let audioChoice = $state<'bit-perfect' | 'standard' | 'later' | null>(null);
	let audioApplyError = $state('');
	const isWindows = typeof navigator !== 'undefined' && /Win/i.test(navigator.platform);

	onMount(async () => {
		// `?preview` bypasses the auto-redirect so the page can be designed/QA'd
		// without unsetting onboarding state on the backend.
		const recovery = recoverAppearanceDraft();
		if (recovery) { appearanceDraft = recovery; step = 5; completeError = 'Your appearance choices are ready to save. Retry to finish setup.'; return; }
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

			return resp.ok;
		} catch {
			return false;
		}
	}

	function discardAppearance() {
		appearanceDraft = draftAppearance(savedAppearance);
		clearAppearanceRecovery();
	}
	async function skipAll() {
		discardAppearance();
		step = 5;
		await finish();
	}
	async function startBackgroundSync() {
		if (syncRequested || syncRequestPending) return;
		syncRequestPending = true; syncRequestError = '';
		try {
			const response = await startTidalSync();
			const result = await response.json().catch(() => ({}));
			if (!response.ok || result.status !== 'sync_started') throw new Error(result.message ?? 'Library sync could not start. Retry in Settings → Library.');
			syncRequested = true;
		} catch (error) { syncRequestError = error instanceof Error ? error.message : 'Library sync could not start.'; }
		finally { syncRequestPending = false; }
	}
	function handleTidalConnected() {
		tidalConnected = true;
		void startBackgroundSync();
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
		if (completing) return;
		completing = true; completeError = '';
		try {
			rememberAppearanceDraft(appearanceDraft);
			if (!await markComplete()) throw new Error("Couldn't save your setup. Your choices are retained.");
			persistAppearanceDraft(appearanceDraft);
			const changes = appearanceDraft.changes;
			if (changes.palette) palette.set(changes.palette);
			if (changes.theme) surfaceMode.set(changes.theme);
			if (changes.background) setWallpaper(changes.background);
			clearAppearanceRecovery();
			try { markLocalOnboardingComplete(getStoredToken()); } catch { /* Server completion is authoritative. */ }
			await goto('/', { replaceState: true });
		} catch (error) { completeError = error instanceof Error ? error.message : "Couldn't finish setup."; }
		finally { completing = false; }
	}
	async function continueAnyway() {
		discardAppearance();
		await goto('/', { replaceState: true });
	}
</script>

<svelte:head>
	<title>Welcome — NOORwave</title>
</svelte:head>

<div bind:this={brandFrame} class="onboarding onboarding-scope" data-theme="dark">
	<div class="stack">
		<img class="wordmark wordmark-on-dark" src="/noor-logo-centered.svg" alt="NOORwave" />
		<div class="card glass-panel">
		<div class="progress" aria-label="Onboarding steps">
			{#each Array(6) as _, i}
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
				<h1 class="onboarding-title">Welcome.</h1>
				<p class="onboarding-lede">Pure sound. Perfect flow.</p>
				<button class="btn btn-primary onboarding-action" onclick={() => (step = 1)}>Get started</button>
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
				{#if $syncStatus === 'syncing' && !syncErrorMessage}
					<p class="footnote">Library syncing in the background… {$syncProgress ?? 0}%</p>
				{:else if syncErrorMessage}
					<p class="footnote warn">{syncErrorMessage}</p>
				{:else if syncRequestPending || (syncRequested && $syncStatus === 'idle')}
					<p class="footnote">Sync requested; waiting for progress.</p>
				{/if}
			</div>
		{:else if step === 3}
			<div class="step audio-quality">
				<h2 class="onboarding-title">How should we play it?</h2>
				<p class="onboarding-lede">Choose your output. You can change this anytime in Settings.</p>
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
						<span class="audio-choice-body">Keep current defaults. You can change this in Settings → Playback anytime.</span>
					</button>
				</div>
				{#if audioApplyError}
					<p class="error" role="alert">{audioApplyError}</p>
				{/if}
			</div>
		{:else if step === 4}
			<div class="step appearance-step settings-scope">
				<h2 class="onboarding-title">Choose your look</h2>
				<AppearancePreview values={appearanceDraft.values} />
				<div class="appearance-controls glass-tile">
					<AppearanceFields values={appearanceDraft.values} onchange={(changes) => appearanceDraft = changeAppearanceDraft(appearanceDraft, changes)} prefix="onboarding" />
				</div>
				<div class="onboarding-actions"><button class="btn btn-primary onboarding-action" onclick={() => step = 5}>Continue</button><button class="btn btn-glass onboarding-action" onclick={() => { discardAppearance(); step = 5; }}>Skip for now</button></div>
			</div>
		{:else if step === 5}
			<div class="step done">
				<h2 class="onboarding-title">Ready to explore</h2>
				<div class="completion-notice">
					<strong>NOORwave grows with you.</strong>
					<p>Things may look a little bare at first. As you play more music and build your library, discovery becomes more tailored to you and more spaces fill out.</p>
					<p>Connect Last.fm to unlock discovery panels based on your listening profile.</p>
				</div>
				{#if $syncStatus === 'syncing'}<p class="footnote">Library syncing… {$syncProgress ?? 0}%</p>{:else if $syncStatus === 'done'}<p class="footnote">Library sync completed.</p>{/if}
				{#if syncErrorMessage}
					<p class="warn">{syncErrorMessage}</p>
				{:else if syncRequestPending || (syncRequested && $syncStatus === 'idle')}
					<p class="footnote">Sync requested; waiting for progress.</p>
				{/if}
				{#if completeError}
					<p class="error" role="alert">{completeError}</p>
					<div class="onboarding-actions">
						<button class="btn btn-primary onboarding-action" onclick={finish} disabled={completing}>Try again</button>
						<button class="link" onclick={continueAnyway}>Continue anyway</button>
					</div>
				{:else}
					<button class="btn btn-primary onboarding-action" onclick={finish} disabled={completing}>
						{completing ? 'Saving…' : 'Open NOORwave'}
					</button>
				{/if}
			</div>
		{/if}
	</div>
	</div>
</div>

<style>
	.onboarding {
		position: fixed;
		inset: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		/* The layout owns the persistent standing-wave wallpaper. */
		background: transparent;
		color: var(--text-primary);
		font-family: var(--font-body);
		padding: var(--space-6) var(--space-4);
		overflow: auto;
		scrollbar-gutter: stable both-edges;
	}
	.stack {
		position: relative;
		width: 100%;
		max-width: 720px;
		margin-block: auto;
		flex-shrink: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--space-4);
	}
	.wordmark { width: clamp(180px, 24vw, 240px); height: auto; }
	.card {
		position: relative;
		z-index: 1;
		width: 100%;
		padding: var(--space-6);
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
	}
	.progress { display: flex; gap: var(--space-2); justify-content: center; }
	.dot {
		position: relative;
		width: 24px;
		height: 4px;
		border-radius: 999px;
		background: var(--border-muted);
		border: none;
		padding: 0;
		cursor: pointer;
		transition: background var(--motion-fast);
	}
	/* Expand the previous-step target without enlarging the progress marks. */
	.dot::before { content: ''; position: absolute; inset: -20px 0; }
	.dot:disabled { cursor: default; }
	.dot.active { background: var(--accent-line); }
	.dot.current, .dot:not(:disabled):hover { background: var(--accent); }
	.dot:focus-visible { outline: 2px solid var(--accent); outline-offset: 4px; }
	.step {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--space-5);
		min-height: 280px;
		justify-content: center;
		text-align: center;
		width: 100%;
	}
	.completion-notice {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		max-width: 48ch;
		text-align: center;
		font-size: var(--font-size-md);
		line-height: var(--line-height-loose);
		color: var(--text-secondary);
	}
	.completion-notice p { margin: 0; }
	.completion-notice strong { color: var(--text-primary); font-weight: var(--font-weight-semibold); }
	.appearance-controls { width: 100%; padding: var(--space-4); text-align: left; }
	.link {
		background: none;
		border: none;
		color: var(--text-secondary);
		font: inherit;
		font-size: var(--font-size-sm);
		cursor: pointer;
		min-height: 44px;
		padding: var(--space-2);
		text-decoration: underline;
		text-decoration-color: var(--border-strong);
		text-underline-offset: 3px;
		transition: color var(--motion-fast);
	}
	.link:hover { color: var(--text-primary); }
	.link:disabled { opacity: 0.5; cursor: not-allowed; }
	.footnote { margin: 0; font-size: var(--font-size-xs); color: var(--text-secondary); }
	.warn { color: var(--state-warning); }
	.error { color: var(--state-error); margin: 0; }
	.audio-choices { display: flex; flex-direction: column; gap: var(--space-3); width: 100%; max-width: 48ch; }
	.audio-choice {
		text-align: left;
		background: var(--panel-bg);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-sm);
		padding: var(--space-3) var(--space-4);
		cursor: pointer;
		color: inherit;
		font: inherit;
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		transition: background var(--motion-fast), border-color var(--motion-fast);
	}
	.audio-choice:hover { background: var(--bg-hover); border-color: var(--border-strong); }
	.audio-choice:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
	.audio-choice.selected { border-color: var(--accent-line); background: var(--accent-soft); }
	.audio-choice.subtle { background: transparent; }
	.audio-choice-title {
		font-weight: var(--font-weight-semibold);
		font-size: var(--font-size-sm);
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--space-2);
	}
	.audio-choice-pill {
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-medium);
		padding: var(--space-1) var(--space-2);
		border-radius: 999px;
		background: var(--accent-soft);
		color: var(--text-primary);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.audio-choice-body { font-size: var(--font-size-sm); color: var(--text-secondary); line-height: var(--line-height-normal); }
	@media (max-width: 640px) {
		.card { padding: var(--space-4); }
		.appearance-controls :global(.setting-row) { grid-template-columns: minmax(0, 1fr); gap: var(--space-2); }
		.appearance-controls :global(.setting-control) { justify-content: flex-start; flex-wrap: wrap; }
	}
</style>
