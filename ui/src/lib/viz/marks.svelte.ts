/**
 * What is marked, by link: the ids of a geometry's nodes, coordinate systems or elements, which
 * the views of the same link share (`marks` and `mark_sets` in schema/PROTOCOL.md). A row
 * selected in the node table, a node clicked in the 3-D view and a node's, a system's or an
 * element's row clicked in the project tree each mark it in the others. The marks are the
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
