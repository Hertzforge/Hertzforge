import { describe, expect, it } from 'vitest';
import { colormap } from './colormap';
import { cellAt, cellText, inkOn, matrixLayout, FEWEST_CELLS, isZoomed, panWindow, wholeWindow, zoomWindow } from './matrix';

describe('matrixLayout', () => {
	it('makes square cells centered in the space left by the labels', () => {
		const l = matrixLayout(600, 400, 5, 5, 80, 40);
		expect(l.cellW).toBe(l.cellH);
		expect(l.cellH).toBeCloseTo((400 - 40 - 8) / 5, 9);
		const right = 600 - 8;
		expect(l.x0 - 80).toBeCloseTo(right - (l.x0 + 5 * l.cellW), 9);
	});

	it('stretches cells when squares would leave most of the box empty', () => {
		const l = matrixLayout(800, 300, 2, 30, 80, 40);
		expect(l.cellW).not.toBe(l.cellH);
		expect(l.x0).toBe(80);
		expect(l.columns * l.cellW).toBeCloseTo(800 - 80 - 8, 9);
	});
});

describe('cellAt', () => {
	const l = { x0: 100, y0: 10, cellW: 20, cellH: 10, rows: 3, columns: 2 };

	it('finds the row from the top and the column from the left', () => {
		expect(cellAt(100, 10, l)).toEqual({ row: 0, column: 0 });
		expect(cellAt(139.9, 39.9, l)).toEqual({ row: 2, column: 1 });
	});

	it('is null outside the grid', () => {
		expect(cellAt(99.9, 15, l)).toBeNull();
		expect(cellAt(140, 15, l)).toBeNull();
		expect(cellAt(110, 40, l)).toBeNull();
	});
});

describe('inkOn', () => {
	it('is light on the dark end of the colormap and dark on the bright end', () => {
		expect(inkOn(colormap(0))).toBe('light');
		expect(inkOn(colormap(1))).toBe('dark');
	});
});

describe('cellText', () => {
	it('writes two decimals, and a dash for no value', () => {
		expect(cellText(0.9234)).toBe('0.92');
		expect(cellText(1)).toBe('1.00');
		expect(cellText(Number.NaN)).toBe('–');
	});
});

describe('a matrix zoomed and panned', () => {
	const whole = wholeWindow(40, 60);
	const both = { rows: true, columns: true };

	it('zooms about the point under the pointer, which stays where it is', () => {
		const at = { row: 10, column: 45 };
		const win = zoomWindow(whole, 40, 60, 0.5, at, both);
		expect(win.r1 - win.r0).toBeCloseTo(20);
		expect(win.c1 - win.c0).toBeCloseTo(30);
		// the point's share of the window is what it was
		expect((at.row - win.r0) / (win.r1 - win.r0)).toBeCloseTo(10 / 40);
		expect((at.column - win.c0) / (win.c1 - win.c0)).toBeCloseTo(45 / 60);
		expect(isZoomed(win, 40, 60)).toBe(true);
		expect(isZoomed(whole, 40, 60)).toBe(false);
	});

	it('zooms one axis alone over that axis', () => {
		const rows = zoomWindow(whole, 40, 60, 0.5, { row: 20, column: 30 }, { rows: true, columns: false });
		expect([rows.c0, rows.c1]).toEqual([0, 60]);
		expect(rows.r1 - rows.r0).toBeCloseTo(20);
		const columns = zoomWindow(whole, 40, 60, 0.5, { row: 20, column: 30 }, { rows: false, columns: true });
		expect([columns.r0, columns.r1]).toEqual([0, 40]);
		expect(columns.c1 - columns.c0).toBeCloseTo(30);
	});

	it('stops at the whole matrix going out and at a few cells going in', () => {
		expect(zoomWindow(whole, 40, 60, 5, { row: 3, column: 3 }, both)).toEqual(whole);
		const tight = zoomWindow(whole, 40, 60, 1e-6, { row: 39.5, column: 0.2 }, both);
		expect(tight.r1 - tight.r0).toBeCloseTo(FEWEST_CELLS);
		expect(tight.c1 - tight.c0).toBeCloseTo(FEWEST_CELLS);
		expect(tight.r1).toBeLessThanOrEqual(40);
		expect(tight.c0).toBeGreaterThanOrEqual(0);
		// a matrix of one row has nothing to zoom on its rows
		expect(zoomWindow(wholeWindow(1, 8), 1, 8, 0.1, { row: 0.5, column: 4 }, both).r1).toBe(1);
	});

	it('pans by cells and stops at the edges', () => {
		const win = zoomWindow(whole, 40, 60, 0.25, { row: 20, column: 30 }, both);
		const moved = panWindow(win, 40, 60, 3, -4);
		expect(moved.r0).toBeCloseTo(win.r0 + 3);
		expect(moved.c0).toBeCloseTo(win.c0 - 4);
		expect(moved.r1 - moved.r0).toBeCloseTo(win.r1 - win.r0);
		const far = panWindow(win, 40, 60, 1000, -1000);
		expect(far.r1).toBeCloseTo(40);
		expect(far.c0).toBe(0);
		expect(panWindow(whole, 40, 60, 5, 5)).toEqual(whole);
	});

	it('finds the cell under a point in the part shown, and none off it', () => {
		// rows 10 to 20 and columns 30 to 40 drawn 20 px a cell from (100, 8)
		const layout = { x0: 100, y0: 8, cellW: 20, cellH: 20, rows: 40, columns: 60, shown: { r0: 10, r1: 20, c0: 30, c1: 40 } };
		expect(cellAt(110, 18, layout)).toEqual({ row: 10, column: 30 });
		expect(cellAt(100 + 9.5 * 20, 8 + 9.5 * 20, layout)).toEqual({ row: 19, column: 39 });
		expect(cellAt(90, 18, layout)).toBeNull();
		expect(cellAt(100 + 10.5 * 20, 18, layout)).toBeNull();
	});
});
