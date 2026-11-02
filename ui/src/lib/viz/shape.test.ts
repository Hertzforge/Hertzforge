import { describe, expect, it } from 'vitest';
import { nodeTravel } from './shape';

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
