// Relocate an element to `document.body` (or another target) so a
// `position: fixed` modal is positioned against the real viewport.
//
// Why this is needed: an ancestor with `transform`, `filter`, `backdrop-filter`,
// `perspective`, or `will-change: transform` establishes a containing block for
// fixed descendants (CSS spec). The app shell sets `transform: translateZ(0)` and,
// when a wallpaper is active, the scrolling `.workspace` gets a `backdrop-filter`.
// A fixed modal rendered inside the page is therefore positioned relative to the
// scrolling workspace, so it lands at the content's top origin and appears to
// "jump to the top of the page" once you've scrolled down. Portalling the modal
// out of the shell sidesteps every such ancestor.
//
// The default target is SvelteKit's root container (#app-root in app.html), not
// <body>: SvelteKit intercepts link clicks on that container only, so an <a>
// portalled to <body> would bypass the router and reload the whole app. The
// container is `display: contents`, so it adds no box or containing block.
export function portal(node: HTMLElement, target: HTMLElement | string = '#app-root') {
	let resolved: HTMLElement | null = null;

	function mount(t: HTMLElement | string) {
		resolved = typeof t === 'string' ? document.querySelector<HTMLElement>(t) : t;
		(resolved ?? document.body).appendChild(node);
	}

	mount(target);

	return {
		update(t: HTMLElement | string) {
			mount(t);
		},
		destroy() {
			node.parentNode?.removeChild(node);
		},
	};
}
