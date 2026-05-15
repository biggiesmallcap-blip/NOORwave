/// <reference no-default-lib="true"/>
/// <reference lib="esnext" />
/// <reference lib="webworker" />
/// <reference types="@sveltejs/kit" />

import { build, files, version } from '$service-worker';

const worker = self as unknown as ServiceWorkerGlobalScope;
const CACHE = `noor-remote-${version}`;
const META = 'noor-remote-meta';
const PREVIOUS_KEY = '/__previous_cache__';
const ASSETS = [...build, ...files];

worker.addEventListener('install', (event) => {
	event.waitUntil(
		caches.open(CACHE).then((cache) => cache.addAll(ASSETS))
	);
	// Take over from the previous SW immediately so a rebuild's network-first
	// HTML always serves fresh, but DO NOT drop the previous cache here — that
	// happens in `activate` once we've recorded which one to keep.
	worker.skipWaiting();
});

worker.addEventListener('activate', (event) => {
	event.waitUntil(
		(async () => {
			// Remember which cache the previous SW was serving so its hashed
			// chunks remain available for any client still running the old
			// bundle. Without this, an immediate skipWaiting + claim drops the
			// old cache, the old page lazy-imports an old chunk, the new SW
			// can't find it (not in current ASSETS), and the request 404s
			// against the new build that no longer ships the old hash.
			const meta = await caches.open(META);
			const prevResp = await meta.match(PREVIOUS_KEY);
			const previousCache = prevResp ? await prevResp.text() : null;

			const keys = await caches.keys();
			await Promise.all(
				keys.map(async (key) => {
					if (key === CACHE) return;
					if (key === META) return;
					if (key === previousCache) return;
					await caches.delete(key);
				})
			);

			// Future upgrades treat THIS version's cache as the "previous"
			// that the NEXT activate must preserve.
			await meta.put(PREVIOUS_KEY, new Response(CACHE));

			await worker.clients.claim();
		})()
	);
});

worker.addEventListener('fetch', (event) => {
	if (event.request.method !== 'GET') return;
	const url = new URL(event.request.url);
	if (url.origin !== worker.location.origin) return;

	// HTML navigations — network-first so a rebuild with new chunk hashes is
	// picked up immediately. Falls back to whatever HTML the cache last saw
	// when offline.
	if (event.request.mode === 'navigate') {
		event.respondWith(
			fetch(event.request)
				.then((response) => {
					const copy = response.clone();
					void caches.open(CACHE).then((cache) => cache.put(event.request, copy));
					return response;
				})
				.catch(async () => {
					const cache = await caches.open(CACHE);
					return (
						(await cache.match(event.request)) ??
						(await cache.match('/')) ??
						Response.error()
					);
				})
		);
		return;
	}

	// Same-origin static GETs — check every cache (current + retained previous)
	// before going to network. This lets the active SW serve hashed chunks for
	// clients still running the prior bundle after a skipWaiting upgrade. Falls
	// through to network when nothing matches; backfills the current cache so
	// repeated requests don't keep hitting the network.
	event.respondWith(
		(async () => {
			const cached = await caches.match(event.request);
			if (cached) return cached;
			try {
				const response = await fetch(event.request);
				if (response.ok && ASSETS.includes(url.pathname)) {
					const cache = await caches.open(CACHE);
					void cache.put(event.request, response.clone());
				}
				return response;
			} catch {
				return Response.error();
			}
		})()
	);
});
