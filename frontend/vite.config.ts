import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [sveltekit()],
	build: {
		// hls.js is dynamically imported by VideoPlayer.svelte and lands in its own
		// chunk (~523 kB raw / 162 kB gzip). It only loads when the user streams HLS,
		// so the size is fine — but it naturally exceeds Vite's default 500 kB limit.
		// Raise the bar to 600 so the warning still fires on any new chunk that grows.
		chunkSizeWarningLimit: 600,
		rollupOptions: {
			output: {
				manualChunks: (id) => {
					if (id.includes('node_modules/hls.js')) return 'hls';
					if (id.includes('node_modules/@tauri-apps')) return 'tauri';
				}
			}
		}
	}
});
