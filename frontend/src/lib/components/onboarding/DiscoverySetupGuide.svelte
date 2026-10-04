<script lang="ts">
	import { onMount, tick, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { authFetch, getApiBase } from '$lib/api/client';
	import { wsMessages } from '$lib/api/ws';
	import { settingsHref } from '$lib/components/settings/settingsManifest';
	let { enabled }: { enabled: boolean } = $props();
	let visible = $state(false);
	let checking = false;
	let destroyed = false;
	let reservation: { account: string; owner: string } | null = null;
	let acknowledged = false;
	let displayed = false;
	let displayedScope = '';
	const shownScopes = new Set<string>();
	let lastAttempt = 0;

	function canShow() {
		return enabled && document.visibilityState === 'visible'
			&& !document.querySelector('[aria-modal="true"], .modal-backdrop, .patch-info-backdrop');
	}
	async function action(kind: 'reserve' | 'shown' | 'release', claim: { account: string; owner: string }) {
		const response = await authFetch(getApiBase() + '/api/setup/discovery', {
			method: 'POST', headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ action: kind, account_id: claim.account, owner: claim.owner }),
		});
		if (!response.ok) throw new Error('Discovery guidance is unavailable.');
		return (await response.json() as { accepted: boolean }).accepted;
	}
	async function acknowledge() {
		if (!reservation || acknowledged || !visible) return;
		if (await action('shown', reservation)) acknowledged = true;
	}
	async function reconcile(force = false) {
		if (destroyed || checking || !canShow()) return;
		if (!force && Date.now() - lastAttempt < 5000) return;
		checking = true; lastAttempt = Date.now();
		try {
			const response = await authFetch(getApiBase() + '/api/setup/discovery');
			if (!response.ok) return;
			const status = await response.json() as { library_id: string; eligible: boolean; account_id: string | null };
			const scope = status.library_id + ':' + status.account_id;
			if (displayed && scope !== displayedScope) {
				visible = false; displayed = false; reservation = null; acknowledged = false;
			}
			if (displayed && scope === displayedScope) {
				if (!acknowledged && reservation) {
					// Retry a lost display acknowledgement without showing another prompt.
					if (await action('reserve', reservation)) acknowledged = await action('shown', reservation);
				}
				return;
			}
			if (shownScopes.has(scope) || !status.eligible || !status.account_id || !canShow() || destroyed) return;
			const owner = crypto.randomUUID?.() ?? Date.now().toString(36) + '-' + Math.random().toString(36).slice(2);
			const claim = { account: status.account_id, owner };
			if (!await action('reserve', claim)) return;
			reservation = claim; acknowledged = false;
			if (!canShow() || destroyed) { await action('release', claim); reservation = null; return; }
			visible = true;
			await tick(); // Consume guidance only after its actual first display.
			if (visible && !destroyed) { displayed = true; displayedScope = scope; shownScopes.add(scope); await acknowledge(); }
		} catch { /* Retry after reconnect/focus; a server outage shouldn't interrupt listening. */ }
		finally { checking = false; }
	}
	async function dismiss(openSettings: boolean) {
		try { await acknowledge(); } catch { /* A lost response is safe to retry. */ }
		visible = false;
		if (!acknowledged && reservation) {
			// Keep the claim until the server returns so dismissal remains one-time.
			const claim = reservation;
			void action('shown', claim).catch(() => {});
		}
		if (openSettings) await goto(settingsHref('library', 'discovery-engine'));
	}
	$effect(() => {
		const ready = enabled;
		page.url.pathname; // Reconcile when leaving setup or entering the app.
		untrack(() => {
			if (ready) void reconcile(true);
			else visible = false;
		});
	});
	onMount(() => {
		const unsubscribe = wsMessages.subscribe((messages) => {
			const message = messages.at(-1);
			if (message?.type === 'connected' || message?.type === 'library_synced') void reconcile(true);
		});
		const onFocus = () => void reconcile(true);
		window.addEventListener('focus', onFocus);
		document.addEventListener('visibilitychange', onFocus);
		const timer = setInterval(() => void reconcile(), 30_000);
		void reconcile(true);
		return () => {
			destroyed = true; unsubscribe(); clearInterval(timer);
			window.removeEventListener('focus', onFocus);
			document.removeEventListener('visibilitychange', onFocus);
			if (reservation && !displayed) void action('release', reservation).catch(() => {});
		};
	});
</script>
{#if visible && enabled}
	<aside class="discovery-guide" aria-labelledby="discovery-guide-title" aria-live="polite">
		<h2 id="discovery-guide-title">Your library is ready. Build better discovery.</h2>
		<p>Train the discovery engine to find connections across your music. It can start now and benefits from more listening history. Refresh it later as your library and listening grow.</p>
		<div class="guide-actions"><button class="btn btn-primary" onclick={() => void dismiss(true)}>Set up discovery</button><button class="btn btn-glass" onclick={() => void dismiss(false)}>Later</button></div>
	</aside>
{/if}
<style>
	.discovery-guide { position: fixed; z-index: 190; top: var(--space-5); right: var(--space-5); max-width: 420px; margin-left: var(--space-5); padding: var(--space-5); background: var(--bg-elevated); border: 1px solid var(--border-muted); border-radius: var(--radius-lg); box-shadow: var(--shadow-lg); }
	h2 { margin: 0; font-size: var(--font-size-lg); font-weight: var(--font-weight-semibold); line-height: var(--line-height-snug); }
	p { font-size: var(--font-size-sm); color: var(--text-secondary); line-height: var(--line-height-normal); margin: var(--space-3) 0; }
	.guide-actions { display: flex; gap: var(--space-2); flex-wrap: wrap; }
	.guide-actions button { min-height: 44px; }
</style>
