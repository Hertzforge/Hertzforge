import { describe, expect, it } from 'vitest';
import { colormap } from './colormap';
import { cellAt, cellText, inkOn, matrixLayout } from './matrix';

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
