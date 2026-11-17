import { describe, expect, it } from 'vitest';
import { linearColors, span } from './points';

describe('span', () => {
	it('covers the nodes and every point layer together', () => {
		const nodes = new Float32Array([0, 0, 0, 1, 2, 3]);
		const cloud = new Float32Array([-4, 1, 1, 0.5, 9, -2]);
		expect(span([nodes, cloud])).toEqual({ lo: [-4, 0, -2], hi: [1, 9, 3] });
	});

	it('is the points alone when the view has no nodes', () => {
		expect(span([new Float32Array(0), new Float32Array([5, 6, 7])])).toEqual({ lo: [5, 6, 7], hi: [5, 6, 7] });
	});

	it('is infinite when nothing holds a point', () => {
		const { lo, hi } = span([new Float32Array(0)]);
		expect(lo.every((v) => v === Infinity) && hi.every((v) => v === -Infinity)).toBe(true);
	});
});

describe('linearColors', () => {
	it('takes sRGB bytes to linear floats', () => {
		const out = linearColors(new Uint8Array([0, 255, 128]));
		expect(out[0]).toBe(0);
		expect(out[1]).toBe(1);
		expect(out[2]).toBeCloseTo(0.2158, 3);
	});
});
