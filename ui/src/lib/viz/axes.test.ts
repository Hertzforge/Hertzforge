import { describe, expect, it } from 'vitest';
import { along, labeledEdges, niceTicks, type Point } from './axes';

describe('niceTicks', () => {
	it('steps by 1, 2 or 5 times a power of ten and writes the decimals the step needs', () => {
		expect(niceTicks(0, 0.7)).toEqual({ values: [0, 0.2, 0.4, 0.6], decimals: 1 });
		expect(niceTicks(-8, 8)).toEqual({ values: [-5, 0, 5], decimals: 0 });
		expect(niceTicks(-3.2, 3.2)).toEqual({ values: [-2, 0, 2], decimals: 0 });
		expect(niceTicks(-0.041, 0.038)).toEqual({ values: [-0.04, -0.02, 0, 0.02], decimals: 2 });
		expect(niceTicks(0, 1234)).toEqual({ values: [0, 200, 400, 600, 800, 1000, 1200], decimals: 0 });
	});
	it('keeps the ticks inside the range and copes with a flat one', () => {
		const { values } = niceTicks(0.13, 0.47);
		expect(values[0]).toBeGreaterThanOrEqual(0.13);
		expect(values.at(-1)).toBeLessThanOrEqual(0.47);
		expect(niceTicks(2, 2)).toEqual({ values: [2], decimals: 0 });
	});
});

describe('labeledEdges', () => {
	// a unit cube seen face on, the picture's y down: x along the screen, y up the screen, z toward the viewer (no offset)
	const corners: Point[] = [];
	for (let i = 0; i < 8; i++) corners.push({ x: 100 + (i & 1 ? 50 : -50) + (i & 4 ? 10 : 0), y: 100 + (i & 2 ? -50 : 50) + (i & 4 ? -10 : 0) });
	const centre = { x: 105, y: 95 };
	it('labels one edge per axis, the lowest and outermost, with its labels pointing outward', () => {
		const [x, y, z] = labeledEdges(corners, centre);
		expect(x.to - x.from).toBe(1);
		expect(y.to - y.from).toBe(2);
		expect(z.to - z.from).toBe(4);
		// the x edge picked is the one at the bottom front (y low, z low): its middle is lowest on the picture
		expect(x.from).toBe(0);
		expect(x.out.y).toBeGreaterThan(0);
		expect(Math.hypot(x.out.x, x.out.y)).toBeCloseTo(1);
	});
});

describe('along', () => {
	it('places a value on the edge in proportion', () => {
		expect(along({ x: 0, y: 0 }, { x: 10, y: 20 }, -1, 1, 0)).toEqual({ x: 5, y: 10 });
		expect(along({ x: 0, y: 0 }, { x: 10, y: 20 }, 2, 2, 2)).toEqual({ x: 5, y: 10 });
	});
});
