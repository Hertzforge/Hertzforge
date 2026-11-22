/**
 * Elements dragged in the project tree onto an element group, kept out of the component so
 * it can be tested: which elements a drag carries, and whether letting go moves any of them.
 */

/** The elements a drag from element `id` carries: the ones marked, when it is among them, else itself. */
export function draggedElements(marked: readonly number[], id: number): number[] {
	return marked.includes(id) ? [...marked] : [id];
}

/** Whether letting go on `group` moves any of `ids`: some are not among the elements the group lists already. */
export function movesInto(groups: readonly { id: number; elements?: number[] }[], ids: readonly number[], group: number): boolean {
	const there = new Set(groups.find((g) => g.id === group)?.elements ?? []);
	return ids.some((id) => !there.has(id));
}
