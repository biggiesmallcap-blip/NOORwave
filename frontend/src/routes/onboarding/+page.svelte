<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { authFetch, getApiBase } from '$lib/api/client';
	import TidalConnect from '$lib/components/onboarding/TidalConnect.svelte';
	import LastfmConnect from '$lib/components/onboarding/LastfmConnect.svelte';

	let step = $state(0);
	let tidalConnected = $state(false);
	let syncErrorMessage = $state('');
	let completing = $state(false);
	let completeError = $state('');

	onMount(async () => {
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

	function handleLastfmDone() {
		step = 3;
	}

	async function finish() {
		completing = true;
		completeError = '';
		const ok = await markComplete();
		completing = false;
		if (ok) {
			await goto('/', { replaceState: true });
		} else {
			completeError = "Couldn't save your setup.";
		}
	}

	async function continueAnyway() {
		await goto('/', { replaceState: true });
	}
</script>

<svelte:head>
	<title>Welcome — NOORwave</title>
</svelte:head>

<div class="onboarding">
	<div class="card">
		<div class="progress" role="tablist" aria-label="Onboarding steps">
			{#each Array(4) as _, i}
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
				<img class="wordmark" src="/wordmark-animated-dark.svg" alt="NOORwave" />
				<h1>Welcome.</h1>
				<p class="lede">A quiet, focused way to listen to your TIDAL library — with smart radio, trending shelves, and the kind of metadata you'd actually trust.</p>
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
				<LastfmConnect
					variant="onboarding"
					showSkip={true}
					onconnected={handleLastfmDone}
					onskip={handleLastfmDone}
				/>
				{#if tidalConnected && !syncErrorMessage}
					<p class="footnote">Library syncing in the background…</p>
				{:else if syncErrorMessage}
					<p class="footnote warn">{syncErrorMessage}</p>
				{/if}
			</div>
		{:else if step === 3}
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
					<p class="error">{completeError}</p>
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

<style>
	:global(html, body) {
		margin: 0;
		height: 100%;
	}
	.onboarding {
		position: fixed;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		background: radial-gradient(circle at 50% 30%, #1a1f2e 0%, #0a0d14 65%, #05070b 100%);
		color: #e7eaf2;
		font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
		padding: 32px;
		overflow: auto;
	}
	.card {
		width: 100%;
		max-width: 520px;
		background: rgba(255, 255, 255, 0.02);
		border: 1px solid rgba(255, 255, 255, 0.06);
		border-radius: 20px;
		padding: 40px 36px 32px;
		display: flex;
		flex-direction: column;
		gap: 24px;
		backdrop-filter: blur(8px);
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
	.dot.current { background: #4a6dd8; }
	.step {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 16px;
		min-height: 240px;
		justify-content: center;
		text-align: center;
	}
	.welcome .wordmark {
		display: block;
		width: clamp(360px, 56vw, 560px);
		height: auto;
		margin: 0 auto 1rem;
		filter: drop-shadow(0 8px 24px rgba(120, 150, 220, 0.15));
	}
	.step h1, .step h2 {
		margin: 0;
		font-size: 26px;
		font-weight: 600;
		letter-spacing: -0.015em;
	}
	.lede {
		margin: 0;
		max-width: 420px;
		color: #b8c0d4;
		line-height: 1.5;
	}
	.btn {
		font: inherit;
		padding: 12px 28px;
		border-radius: 10px;
		border: 1px solid transparent;
		cursor: pointer;
		font-weight: 500;
		transition: background 120ms;
	}
	.btn-primary { background: #4a6dd8; color: #fff; }
	.btn-primary:hover:not(:disabled) { background: #5a7ce8; }
	.btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }
	.link {
		background: none;
		border: none;
		color: #8b93a7;
		font: inherit;
		font-size: 13px;
		cursor: pointer;
		padding: 4px 8px;
		text-decoration: underline;
		text-decoration-color: rgba(139, 147, 167, 0.3);
		text-underline-offset: 3px;
	}
	.link:hover { color: #e7eaf2; }
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
		font-size: 12px;
		color: #8b93a7;
	}
	.warn { color: #d6b06a; }
	.error { color: #ff8a8a; margin: 0; }
</style>
