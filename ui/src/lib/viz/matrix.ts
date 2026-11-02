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

/** The cell under a point, or null outside the grid. */
export function cellAt(x: number, y: number, layout: MatrixLayout): { row: number; column: number } | null {
	const column = Math.floor((x - layout.x0) / layout.cellW);
	const row = Math.floor((y - layout.y0) / layout.cellH);
	if (!(column >= 0 && column < layout.columns && row >= 0 && row < layout.rows)) return null;
	return { row, column };
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
