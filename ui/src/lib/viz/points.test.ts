import { describe, expect, it } from 'vitest';
import { AMP_UNIT, amplitudeUnit, linearColors, span } from './points';

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

describe("the deflection an amplitude of 1 draws", () => {
	// a 10 x 10 x 1.5 plate's corners: its diagonal is sqrt(202.25)
	const nodes = new Float32Array([-5, -5, -0.25, 5, -5, -0.25, 5, 5, 1.25, -5, 5, 1.25]);

	it("is a share of the whole model's diagonal", () => {
		expect(amplitudeUnit({ nodes })).toBeCloseTo(AMP_UNIT * Math.sqrt(202.25), 6);
		expect(AMP_UNIT).toBe(0.075);
	});

	it('is the same when the view is framed on part of the model', () => {
		const framed = { nodes, home: { frame: { lo: [-2.5, -1, 0.25], hi: [2.5, 1, 1.25] } } };
		expect(amplitudeUnit(framed)).toBe(amplitudeUnit({ nodes }));
	});

	it('counts point and segment layers, and is a share of 1 with nothing to measure', () => {
		const far = { nodes, points: [{ xyz: new Float32Array([15, -5, -0.25]) }] };
		expect(amplitudeUnit(far)).toBeCloseTo(AMP_UNIT * Math.sqrt(400 + 100 + 2.25), 5);
		expect(amplitudeUnit({ nodes: new Float32Array(0) })).toBe(AMP_UNIT);
	});
});
