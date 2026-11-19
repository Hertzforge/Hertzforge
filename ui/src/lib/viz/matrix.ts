/** Geometry and ink for the `matrix` view: one colored cell per value, row 0 at the top. */

export interface MatrixLayout {
	/** Left and top of the first cell, in CSS pixels. */
	x0: number;
	y0: number;
	/** Cell width and height, in CSS pixels. */
	cellW: number;
	cellH: number;
	rows: number;
	columns: number;
	/** The part of the matrix drawn, when it is zoomed: the cells sizes are for it, and `x0`, `y0` its corner. */
	shown?: { r0: number; r1: number; c0: number; c1: number };
}

/**
 * Cells fill the box left after the row labels (`left`) and column labels (`bottom`), square when
 * that is not much smaller than filling it, so a 5 x 5 MAC reads as a square.
 */
export function matrixLayout(
	width: number,
	height: number,
	rows: number,
	columns: number,
	left: number,
	bottom: number,
	pad = 8
): MatrixLayout {
	const w = Math.max(0, width - left - pad);
	const h = Math.max(0, height - bottom - pad);
	let cellW = columns ? w / columns : 0;
	let cellH = rows ? h / rows : 0;
	const side = Math.min(cellW, cellH);
	if (side >= 0.6 * Math.max(cellW, cellH)) cellW = cellH = side;
	return { x0: left + (w - cellW * columns) / 2, y0: pad + (h - cellH * rows) / 2, cellW, cellH, rows, columns };
}

/** The cell under a point, or null outside the grid, or outside the part of it a zoom shows. */
export function cellAt(x: number, y: number, layout: MatrixLayout): { row: number; column: number } | null {
	const across = (x - layout.x0) / layout.cellW;
	const down = (y - layout.y0) / layout.cellH;
	const shown = layout.shown;
	if (shown && !(across >= 0 && across <= shown.c1 - shown.c0 && down >= 0 && down <= shown.r1 - shown.r0)) return null;
	const column = Math.floor(across + (shown?.c0 ?? 0));
	const row = Math.floor(down + (shown?.r0 ?? 0));
	if (!(column >= 0 && column < layout.columns && row >= 0 && row < layout.rows)) return null;
	return { row, column };
}

/** The part of a matrix on screen, in cells: rows `r0` to `r1` and columns `c0` to `c1`, which need not be whole. */
export interface MatrixWindow {
	r0: number;
	r1: number;
	c0: number;
	c1: number;
}

/** The whole matrix. */
export function wholeWindow(rows: number, columns: number): MatrixWindow {
	return { r0: 0, r1: rows, c0: 0, c1: columns };
}

/** Whether a window shows less than the whole matrix. */
export function isZoomed(win: MatrixWindow, rows: number, columns: number): boolean {
	return win.r0 > 1e-9 || win.c0 > 1e-9 || win.r1 < rows - 1e-9 || win.c1 < columns - 1e-9;
}

/** The fewest cells a zoom leaves across an axis. */
export const FEWEST_CELLS = 2;

/** One axis from `low` to `high` scaled by `factor` about `at`, kept on 0..`count` and no narrower than `FEWEST_CELLS`. */
function zoomAxis(low: number, high: number, count: number, factor: number, at: number): [number, number] {
	const span = Math.min(count, Math.max(Math.min(FEWEST_CELLS, count), (high - low) * factor));
	const share = high > low ? (at - low) / (high - low) : 0.5;
	const start = Math.max(0, Math.min(count - span, at - share * span));
	return [start, start + span];
}

/**
 * A window zoomed by `factor` (under 1 in, over 1 out) about the cell point `at`, on the rows, the
 * columns or both: the point under the pointer stays where it is, as a data plot's zoom does.
 */
export function zoomWindow(win: MatrixWindow, rows: number, columns: number, factor: number, at: { row: number; column: number }, axes: { rows: boolean; columns: boolean }): MatrixWindow {
	const [r0, r1] = axes.rows ? zoomAxis(win.r0, win.r1, rows, factor, at.row) : [win.r0, win.r1];
	const [c0, c1] = axes.columns ? zoomAxis(win.c0, win.c1, columns, factor, at.column) : [win.c0, win.c1];
	return { r0, r1, c0, c1 };
}

/** A window moved by cells, stopping at the matrix's edges. */
export function panWindow(win: MatrixWindow, rows: number, columns: number, byRows: number, byColumns: number): MatrixWindow {
	const r0 = Math.max(0, Math.min(rows - (win.r1 - win.r0), win.r0 + byRows));
	const c0 = Math.max(0, Math.min(columns - (win.c1 - win.c0), win.c0 + byColumns));
	return { r0, r1: r0 + (win.r1 - win.r0), c0, c1: c0 + (win.c1 - win.c0) };
}

/** Dark or light text on a cell of this color (RGB in 0..1), by its relative luminance. */
export function inkOn(rgb: [number, number, number]): 'dark' | 'light' {
	const linear = rgb.map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
	const luminance = 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
	return luminance > 0.18 ? 'dark' : 'light';
}

/** A value as its cell shows it: two decimals, and a dash where there is none. */
export function cellText(value: number): string {
	return Number.isFinite(value) ? value.toFixed(2) : '–';
}
