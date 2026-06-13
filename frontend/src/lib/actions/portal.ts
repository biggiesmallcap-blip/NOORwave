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
// out to <body> sidesteps every such ancestor.
export function portal(node: HTMLElement, target: HTMLElement | string = 'body') {
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
