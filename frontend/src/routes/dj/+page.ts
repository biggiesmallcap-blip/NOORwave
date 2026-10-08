import { redirect } from '@sveltejs/kit';

// Automix and DJ became one Mix page (design audit, October 2026).
export function load() {
	redirect(308, '/mix');
}
