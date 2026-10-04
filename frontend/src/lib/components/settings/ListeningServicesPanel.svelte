<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api/client';
	import { ListeningServices, lastfmConnectionLabel } from './listeningServices.svelte';
	import { settingsHref } from './settingsManifest';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import ExternalLink from '$lib/components/ui/ExternalLink.svelte';
	import '$lib/components/settings/settings.css';
	let { guided = false, showHistory = true }: { guided?: boolean; showHistory?: boolean } = $props();
	const services = new ListeningServices();
	let key = $state('');
	let secret = $state('');
	let token = $state('');
	let editLastfm = $state(false);
	let editListenBrainz = $state(false);
	let backfillMessage = $state('');
	let backfillError = $state('');
	let backfilling = $state(false);
	onMount(() => { void services.refresh(); });
	async function saveLastfm() {
		if (await services.saveLastfm(key, secret)) { key = ''; secret = ''; editLastfm = false; }
	}
	async function saveListenBrainz() {
		if (await services.saveListenBrainz(token)) { token = ''; editListenBrainz = false; }
	}
	async function removeLastfm() {
		if (confirm('Remove Last.fm credentials? Tag enrichment, profile recommendations and account listening uploads will be unavailable until you set them up again. Existing library tags are retained.')) await services.disconnectLastfm(true);
	}
	async function backfill() {
		if (backfilling || services.busy) return;
		backfilling = true; backfillMessage = ''; backfillError = '';
		try {
			const result = await api.backfillScrobbles();
			if (result.status === 'error') throw new Error('Listening history could not be queued.');
			backfillMessage = result.queued > 0 ? result.queued.toLocaleString() + ' provider submissions queued from the last ' + result.days + ' days.'
				: result.status === 'up_to_date' || (result.providers ?? 0) > 0 ? 'Already queued or submitted; no new uploads.'
				: (result.eligible ?? 0) > 0 ? 'Connect a listening provider before uploading.' : 'No eligible listens in the last ' + result.days + ' days.';
			await services.refresh();
		} catch (error) { backfillError = error instanceof Error ? error.message : 'Upload failed.'; }
		finally { backfilling = false; }
	}
	let providerConnected = $derived(!!services.lastfm?.scrobbling || !!services.listenbrainz?.scrobbling);
	let pending = $derived(Math.max(services.lastfm?.pending_submissions ?? 0, services.listenbrainz?.pending_submissions ?? 0));
	let failed = $derived(Math.max(services.lastfm?.failed_submissions ?? 0, services.listenbrainz?.failed_submissions ?? 0));
</script>

<div class="settings-scope listening-services">
	<section class="glass-tile section-panel" data-setting-id="lastfm-service">
		<header class="provider-header">
			<div><h3>Last.fm</h3>{#if services.lastfm?.scrobbling && !services.lastfmError}<p>@{services.lastfm.user ?? 'Connected account'}</p>{/if}</div>
			<StateBadge label={services.lastfmError ? 'Status unavailable' : services.loading ? 'Checking…' : lastfmConnectionLabel(services.lastfm)} tone={services.lastfmError ? 'error' : services.lastfm?.scrobbling || services.lastfm?.enrichment ? 'success' : 'muted'} compact />
		</header>
		{#if guided && !services.lastfm?.scrobbling}<p class="setting-status">Optional: listening-profile recommendations, scrobbles and loves.</p>{/if}
		{#if services.lastfmError}<p class="error" role="alert">{services.lastfmError}</p><button class="btn btn-glass" onclick={() => void services.refresh()} disabled={services.loading}>Retry status</button>{/if}
		{#if services.lastfmMessage}<p class="setting-status" role="status">{services.lastfmMessage}</p>{/if}
		{#if services.lastfm && !services.loading}
			{#if !services.lastfm.api_key_configured && !editLastfm && !guided}<p class="setting-status">Need a key? <ExternalLink href="https://www.last.fm/api/account/create">Create a Last.fm API application</ExternalLink></p>{/if}
			{#if services.lastfm.scrobbling}
				<div class="action-row"><button class="btn btn-glass" onclick={() => void services.disconnectLastfm()} disabled={!!services.busy}>Disconnect account</button><button class="btn btn-glass" onclick={() => editLastfm = !editLastfm}>Edit credentials</button></div>
			{:else if services.lastfm.api_key_configured && services.lastfm.api_secret_configured}
				<p class="setting-status">Credentials are ready; account approval unlocks profile recommendations.</p>
				<p class="privacy">Opt-in only. Provider profiles may be public; disable other scrobblers to avoid duplicates.</p>
				<div class="action-row">
					{#if services.approvalPending}<button class="btn btn-primary" onclick={() => void services.finishLastfm()} disabled={!!services.busy}>Finish connection</button>
					{:else}<button class="btn btn-primary" onclick={() => void services.startLastfm()} disabled={!!services.busy}>Connect account</button>{/if}
					<button class="btn btn-glass" onclick={() => editLastfm = !editLastfm}>Edit credentials</button>
				</div>
			{:else if !guided && !editLastfm}
				<div class="action-row"><button class="btn btn-glass" onclick={() => editLastfm = true}>{services.lastfm.api_key_configured ? 'Connect account' : 'Set up Last.fm'}</button></div>
			{/if}
			{#if services.authUrl}<div class="approval-link"><ExternalLink href={services.authUrl}>Open Last.fm approval</ExternalLink></div><button class="btn btn-glass" onclick={() => void services.startLastfm()} disabled={!!services.busy}>Restart connection</button>{/if}
			{#if editLastfm || (guided && (!services.lastfm.api_key_configured || !services.lastfm.api_secret_configured))}
				<form onsubmit={(event) => { event.preventDefault(); void saveLastfm(); }}>
					<ol class="setup-help"><li><ExternalLink href="https://www.last.fm/api/account/create">Create a Last.fm API application</ExternalLink> to get a key and shared secret.</li><li>Enter the key below. The shared secret is optional for tags.</li><li>For scrobbling and profile recommendations, add the secret, save, then connect and approve your account.</li></ol>
					<label>API key<input type="password" aria-label="Last.fm API key" bind:value={key} autocomplete="off" placeholder={services.lastfm.api_key_configured ? 'Saved key retained when blank' : ''} /></label>
					<label>Shared secret<input type="password" aria-label="Last.fm shared secret" bind:value={secret} autocomplete="off" placeholder={services.lastfm.api_secret_configured ? 'Saved secret retained when blank' : ''} /></label>
					<p class="privacy">A key alone enables tags. Adding a secret and approving your account opts into listening uploads; profiles may be public. Disable other scrobblers to avoid duplicates.</p>
					<button class="btn btn-primary" disabled={!!services.busy || !(key.trim() || (services.lastfm.api_key_configured && secret.trim()))}>{services.busy === 'lastfm-save' ? 'Saving…' : 'Save credentials'}</button>
					{#if !guided}<button type="button" class="btn btn-glass" onclick={() => editLastfm = false} disabled={!!services.busy}>Cancel</button>{/if}
				</form>
			{/if}
			{#if !guided && services.lastfm.api_key_configured}
				<details><summary>Credential management</summary><button class="btn btn-glass danger" onclick={() => void removeLastfm()} disabled={!!services.busy}>Remove Last.fm credentials</button><a class="btn btn-glass" href={settingsHref('library', 'last-fm-tags')}>Tag enrichment</a></details>
			{/if}
		{/if}
	</section>
	<section class="glass-tile section-panel" data-setting-id="listenbrainz-service">
		<header class="provider-header"><div><h3>ListenBrainz</h3>{#if services.listenbrainz?.scrobbling && !services.listenbrainzError}<p>@{services.listenbrainz.user ?? 'Connected account'}</p>{/if}</div><StateBadge label={services.listenbrainzError ? 'Status unavailable' : services.loading ? 'Checking…' : services.listenbrainz?.scrobbling ? 'Connected' : 'Not connected'} tone={services.listenbrainzError ? 'error' : services.listenbrainz?.scrobbling ? 'success' : 'muted'} compact /></header>
		{#if guided && !services.listenbrainz?.scrobbling}<p class="setting-status">Optional: open listening history and collaborative recommendations.</p>{/if}
		{#if services.listenbrainzError}<p class="error" role="alert">{services.listenbrainzError}</p><button class="btn btn-glass" onclick={() => void services.refresh()} disabled={services.loading}>Retry status</button>{/if}
		{#if services.listenbrainzMessage}<p class="setting-status" role="status">{services.listenbrainzMessage}</p>{/if}
		{#if services.listenbrainz && !services.loading}
			{#if services.listenbrainz.configured}<div class="action-row"><button class="btn btn-glass" onclick={() => void services.disconnectListenBrainz()} disabled={!!services.busy}>Disconnect</button><button class="btn btn-glass" onclick={() => editListenBrainz = !editListenBrainz}>Edit token</button></div>{/if}
			{#if !services.listenbrainz.configured && !guided && !editListenBrainz}<div class="action-row"><button class="btn btn-glass" onclick={() => editListenBrainz = true}>Connect ListenBrainz</button></div>{/if}
			{#if editListenBrainz || (guided && !services.listenbrainz.configured)}
				<form onsubmit={(event) => { event.preventDefault(); void saveListenBrainz(); }}>
					<p class="setting-status"><ExternalLink href="https://listenbrainz.org/profile/">Find your user token</ExternalLink></p>
					<label>User token<input type="password" aria-label="ListenBrainz user token" bind:value={token} autocomplete="off" /></label>
					<p class="privacy">Connecting opts into listening uploads. Your profile may be public; disable other scrobblers to avoid duplicates.</p>
					<button class="btn btn-primary" disabled={!!services.busy || !token.trim()}>{services.busy === 'listenbrainz-save' ? 'Validating…' : 'Connect ListenBrainz'}</button>
					{#if !guided}<button type="button" class="btn btn-glass" onclick={() => editListenBrainz = false} disabled={!!services.busy}>Cancel</button>{/if}
				</form>
			{/if}
		{/if}
	</section>
	{#if showHistory}
		<details class="glass-tile section-panel" data-setting-id="listening-history"><summary>Listening history<span class="disclosure-status">{services.loading ? 'Checking…' : services.lastfmError || services.listenbrainzError || !services.lastfm || !services.listenbrainz ? 'Status unavailable' : pending ? pending + ' pending' : failed ? failed + ' failed' : providerConnected ? 'Up to date' : 'No provider connected'}</span></summary>
			{#if services.loading || !services.lastfm || !services.listenbrainz || services.lastfmError || services.listenbrainzError}
				<p class="setting-status">Refresh to confirm the listening upload queue.</p>
			{:else}<p class="setting-status">{pending} queued · {failed} failed. Eligible listens upload while NOORwave is running.</p>{/if}
			<div class="action-row"><button class="btn btn-glass" onclick={() => void services.refresh()} disabled={services.loading || !!services.busy}>Refresh status</button><button class="btn btn-glass" onclick={() => void backfill()} disabled={!providerConnected || backfilling || !!services.busy}>{backfilling ? 'Queueing…' : 'Upload last 30 days'}</button></div>
			{#if backfillMessage}<p class="setting-status" role="status">{backfillMessage}</p>{/if}{#if backfillError}<p class="error" role="alert">{backfillError}</p>{/if}
		</details>
	{/if}
</div>
<style>
	.listening-services { display: flex; flex-direction: column; gap: var(--space-4); min-width: 0; width: 100%; text-align: left; }
	.provider-header { display: flex; align-items: center; justify-content: space-between; gap: var(--space-3); }
	.provider-header > div { min-width: 0; }
	.provider-header p { overflow-wrap: anywhere; }
	h3 { font-size: var(--font-size-md); margin: 0; font-weight: var(--font-weight-semibold); }
	.provider-header p { margin: var(--space-1) 0; font-size: var(--font-size-sm); color: var(--text-secondary); }
	form { display: flex; flex-direction: column; gap: var(--space-3); margin-top: var(--space-3); }
	form label { display: flex; flex-direction: column; gap: var(--space-1); text-align: left; font-size: var(--font-size-sm); }
	.setup-help { margin: 0; padding-left: var(--space-5); color: var(--text-secondary); font-size: var(--font-size-sm); text-align: left; }
	.privacy { color: var(--text-secondary); font-size: var(--font-size-sm); margin: var(--space-2) 0; }
	.approval-link { display: block; margin: var(--space-3) 0; overflow-wrap: anywhere; }
	@media (max-width: 480px) { .provider-header { align-items: flex-start; flex-wrap: wrap; } }
</style>
