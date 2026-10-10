// Open a <details> panel when a job starts, without ever closing it.
//
// Why not `open={running}`: Svelte groups that property write into the same
// effect as every other dynamic value in the panel, so each status poll
// re-assigns `open = running` and snaps a panel the user opened back shut.
// This only reacts to the idle -> running edge and leaves the user in charge.
export function openWhile(node: HTMLDetailsElement, active: boolean) {
	let previous = active;
	if (active) node.open = true;

	return {
		update(next: boolean) {
			if (next && !previous) node.open = true;
			previous = next;
		},
	};
}
