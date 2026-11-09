import { describe, expect, it } from 'vitest';
import { alignedShape, followed, nodeTravel, STILL } from './shape';

/** The largest distance from the origin of re cos(t) - im sin(t), found by sweeping t. */
function swept(re: number[], im: number[]): number {
	let largest = 0;
	for (let k = 0; k < 3600; k++) {
		const t = (2 * Math.PI * k) / 3600;
		const d = re.map((r, j) => r * Math.cos(t) - im[j] * Math.sin(t));
		largest = Math.max(largest, Math.hypot(...d));
	}
	return largest;
}

describe('nodeTravel', () => {
	it('is the length of each vector for a real shape', () => {
		const out = new Float32Array(2);
		const peak = nodeTravel(new Float32Array([3, 4, 0, 0, 0, -1]), undefined, out);
		expect(Array.from(out)).toEqual([5, 1]);
		expect(peak).toBe(5);
	});

	it('matches a phase sweep for a complex shape', () => {
		const re = [1, 0.3, -0.2];
		const im = [0.4, -0.8, 0.5];
		const out = new Float32Array(1);
		nodeTravel(new Float32Array(re), new Float32Array(im), out);
		expect(out[0]).toBeCloseTo(swept(re, im), 4);
	});

	it('is the circle radius for a node going round in a circle', () => {
		// x = cos t, y = -sin t: a quarter cycle out of phase, the same distance throughout
		const out = new Float32Array(1);
		nodeTravel(new Float32Array([1, 0, 0]), new Float32Array([0, 1, 0]), out);
		expect(out[0]).toBeCloseTo(1, 6);
	});
});

describe('alignedShape', () => {
	it('flips a real shape by a half turn and scales it to the peak', () => {
		const re = new Float32Array(6);
		const im = new Float32Array(6);
		alignedShape(new Float32Array([0, 0, 2, 0, 0, -1]), undefined, Math.PI, 3, re, im);
		expect(Array.from(re).map((v) => +v.toFixed(6))).toEqual([0, 0, -3, 0, 0, 1.5]);
		expect(Math.max(...Array.from(im).map(Math.abs))).toBeLessThan(1e-6);
	});

	it('rotates a complex shape and keeps the ratio between nodes', () => {
		const re = new Float32Array(3);
		const im = new Float32Array(3);
		// 1 + i, rotated by -45 degrees, is sqrt(2) along the real axis
		alignedShape(new Float32Array([0, 0, 1]), new Float32Array([0, 0, 1]), -Math.PI / 4, 2, re, im);
		expect(re[2]).toBeCloseTo(2, 6);
		expect(im[2]).toBeCloseTo(0, 6);
	});
});

describe('followed', () => {
	it("gives each point its node's offset, and none to a point that stays put", () => {
		const source = new Float32Array([1, 2, 3, 4, 5, 6]);
		const out = new Float32Array(9).fill(9);
		followed(source, new Uint32Array([1, STILL, 0]), out);
		expect(Array.from(out)).toEqual([4, 5, 6, 0, 0, 0, 1, 2, 3]);
	});
});
