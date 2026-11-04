/**
* The MAC matrix as 3-D bars, where both height and color are the value, as Visual Dynamics'
* `viz/mac_bars.py` draws it. The flat grid shows which pairs match at a glance; the bars show
* what the grid shows poorly, how much the near matches differ: as heights, a 0.6 next to a 0.9
* is a visible step. The scale is fixed at 0 to 1 because a MAC is a bounded ratio.
*
* Rows are the first set's modes and columns the second's, in a grid of unit cells: x along the
* columns, y along the rows, z the value. Row 0 is nearest the viewer, as the flat grid reads
* down from the top left.
*/

import { LABEL_LIMIT, namedRecords } from './waterfall';

/** Bar footprint within its unit cell; the gap lets a row of near-1.0 bars read as bars. */
export const FOOTPRINT = 0.78;

/** A full MAC of 1.0 stands this fraction of the grid's larger side tall. */
export const HEIGHT = 0.4;

/** A zero cell still shows a plinth this fraction of full height: measured and nothing, not a hole. */
export const PLINTH = 0.004;

/** The committed-match red, the same on the grid and on the bars. */
export const MARK_RED = '#e5534b';

/** The picked-pair blue, for outlines in both readings. */
export const PICK_BLUE = '#0a84ff';

/** Checker squares across a face's short side of a committed match. */
export const CHECKER = 4;

/** How far a mark stands off the bar it marks, in cell units, so it never z-fights the faces. */
export const MARK_LIFT = 0.02;

/** How far the plain black outline stands off its bar: half the marks', so a pick frames outside it. */
export const EDGE_LIFT = MARK_LIFT / 2;

/** The largest grid that gets outlined; past this the outlines close over the field into a dark mat. */
export const OUTLINE_LIMIT = 40;

export { LABEL_LIMIT };

/** How tall a full MAC stands for a `rows` by `columns` grid. */
export function fullHeight(rows: number, columns: number): number {
	return HEIGHT * Math.max(rows, columns);
}

/** A bar's height for `value`, never below the plinth. */
export function barHeight(
	value: number,
	rows: number,
	columns: number
): number {
	const v = Number.isFinite(value) ? Math.min(1, Math.max(0, value)) : 0;
	return Math.max(v, PLINTH) * fullHeight(rows, columns);
}

/** Which rows or columns get a frequency label: every nth, within the limit. */
export function named(count: number): number[] {
	return namedRecords(count, LABEL_LIMIT);
}

/** The bars' home view: from the front left, high enough that a full bar does not hide the far edge. */
export function homeCamera(
	rows: number,
	columns: number
): { position: [number, number, number]; target: [number, number, number] } {
	const extent = Math.max(rows, columns);
	return {
		position: [columns * 0.5 - 1.1 * extent, -1.4 * extent, 1.2 * extent],
		target: [columns * 0.5, rows * 0.42, 0.12 * extent]
	};
}

/** Which (row, column) an instance of the bars mesh is; instances are laid row-major. */
export function pairOf(
	instance: number,
	columns: number
): { row: number; column: number } {
	return { row: Math.floor(instance / columns), column: instance % columns };
}

/**
* The twelve edges of every bar in `cells`, lifted off the faces by `lift`, as line-segment
* endpoints: [x, y, z] pairs, 24 numbers per bar.
*/
export function outlineSegments(
	cells: Iterable<{ row: number; column: number }>,
	heights: (row: number, column: number) => number,
	lift = MARK_LIFT
): Float32Array {
	const margin = (1 - FOOTPRINT) / 2;
	const out: number[] = [];
	const edges: [number, number][] = [
		[0, 1],
		[1, 2],
		[2, 3],
		[3, 0],
		[4, 5],
		[5, 6],
		[6, 7],
		[7, 4],
		[0, 4],
		[1, 5],
		[2, 6],
		[3, 7]
	];
	for (const { row, column } of cells) {
		const x0 = column + margin - lift;
		const x1 = column + margin + FOOTPRINT + lift;
		const y0 = row + margin - lift;
		const y1 = row + margin + FOOTPRINT + lift;
		const z1 = heights(row, column) + lift;
		const corner: [number, number, number][] = [
			[x0, y0, 0],
			[x1, y0, 0],
			[x1, y1, 0],
			[x0, y1, 0],
			[x0, y0, z1],
			[x1, y0, z1],
			[x1, y1, z1],
			[x0, y1, z1]
		];
		for (const [a, b] of edges) out.push(...corner[a], ...corner[b]);
	}
	return new Float32Array(out);
}

/** The picks after a click on `cell`: alone, or toggled among the others with the modifier. */
export function togglePick(
	picks: { row: number; column: number }[],
	cell: { row: number; column: number },
	extend: boolean
): {
	picks: { row: number; column: number }[];
	active: { row: number; column: number } | null;
} {
	const same = (a: { row: number; column: number }) =>
		a.row === cell.row && a.column === cell.column;
	if (!extend) return { picks: [cell], active: cell };
	if (picks.some(same)) {
		const left = picks.filter((p) => !same(p));
		return { picks: left, active: left.length ? left[left.length - 1] : null };
	}
	return { picks: [...picks, cell], active: cell };
}

/** The picked pairs as `project.add_matches` takes them: row,column numbered from 1. */
export function pairsText(picks: { row: number; column: number }[]): string {
	return picks.map((p) => `${p.row + 1},${p.column + 1}`).join('; ');
}
