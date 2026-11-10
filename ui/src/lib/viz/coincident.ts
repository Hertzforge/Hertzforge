/**
 * Nodes that sit at one place: a drive point's two nodes, a mesh's seam, a node numbered twice.
 * The scene captions them together ("104, 200 (2 here)"), so the caption built here
 * names every id at the hovered node's position.
 */

/** Each node's companions at the same position (itself included), by node index; built once per view. */
export function coincidentGroups(nodes: Float32Array): Map<number, number[]> {
	const byPlace = new Map<string, number[]>();
	const count = nodes.length / 3;
	for (let n = 0; n < count; n++) {
		const key = `${nodes[3 * n]},${nodes[3 * n + 1]},${nodes[3 * n + 2]}`;
		const list = byPlace.get(key);
		if (list) list.push(n);
		else byPlace.set(key, [n]);
	}
	const out = new Map<number, number[]>();
	for (const list of byPlace.values()) if (list.length > 1) for (const n of list) out.set(n, list);
	return out;
}

/** The caption for node index `n`: its id, and every other id at the same place with how many are there. */
export function nodeCaption(n: number, ids: Uint32Array | undefined, groups: Map<number, number[]>): string {
	const id = (k: number) => String(ids ? ids[k] : k + 1);
	const here = groups.get(n);
	if (!here) return `node ${id(n)}`;
	return `nodes ${here.map(id).join(', ')} (${here.length} here)`;
}
