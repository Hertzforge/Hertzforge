/**
 * The nodes marked, by link: what a geometry's node table, its 3-D view and its rows in the
 * project tree share (`marks` in schema/PROTOCOL.md). A row selected in the table, a node clicked
 * in the view or a node's row clicked in the tree marks it in all three. The marks are the
 * shell's alone: no operation reads them.
 */

interface Marked {
	ids: number[];
	/** The first marked node's row among the geometry's nodes, when the marker knows it: where a long table scrolls to. */
	row?: number;
}

// raw, so the ids marked come back as the array that was given
let held = $state.raw<Record<string, Marked>>({});
const NONE: number[] = [];

/** The ids marked under `link`. */
export function marked(link: string): number[] {
	return held[link]?.ids ?? NONE;
}

/** The first marked node's row, when whoever marked it said. */
export function markedRow(link: string): number | undefined {
	return held[link]?.row;
}

export function mark(link: string, ids: number[], row?: number) {
	held = { ...held, [link]: { ids, row } };
}
