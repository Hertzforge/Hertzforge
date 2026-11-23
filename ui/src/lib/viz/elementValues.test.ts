import { describe, expect, it } from 'vitest';
import { colormap } from './colormap';
import { cornerRows, paintElements, rowPeak } from './elementValues';

const linear = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);

describe('element values', () => {
	it('gives every triangle corner its own copy of its node', () => {
		const nodes = new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1]);
		const out = cornerRows(nodes, new Uint32Array([3, 1, 2]));
		expect(Array.from(out)).toEqual([0, 0, 1, 1, 0, 0, 0, 1, 0]);
	});

	it('reads the peak of the mode shown, not of every mode', () => {
		// two modes of three elements: the second mode's peak is 9, the first's 2
		const values = new Float32Array([1, -2, 0.5, 9, 3, -4]);
		expect(rowPeak(values, 0, 3)).toBe(2);
		expect(rowPeak(values, 1, 3)).toBe(9);
	});

	it('runs from minus the peak at the low end to plus it at the high end, each element flat', () => {
		const values = new Float32Array([-2, 0, 2]);
		const owner = new Uint32Array([0, 2, 1]);
		const out = new Float32Array(27);
		expect(paintElements(values, 0, 3, owner, out, 0)).toBe(2);
		const at = (t: number) => colormap(t, [0, 0, 0], 0).map(linear);
		const corner = (tri: number, k: number) => Array.from(out.subarray(9 * tri + 3 * k, 9 * tri + 3 * k + 3));
		for (const [tri, t] of [[0, 0], [1, 1], [2, 0.5]] as const)
			for (let k = 0; k < 3; k++) corner(tri, k).forEach((c, i) => expect(c).toBeCloseTo(at(t)[i], 6));
	});

	it('draws a mode with nothing strained at the middle of the map', () => {
		const out = new Float32Array(9);
		expect(paintElements(new Float32Array([0, 0]), 0, 2, new Uint32Array([1]), out, 0)).toBe(0);
		const mid = colormap(0.5, [0, 0, 0], 0).map(linear);
		Array.from(out.subarray(0, 3)).forEach((c, i) => expect(c).toBeCloseTo(mid[i], 6));
	});
});
