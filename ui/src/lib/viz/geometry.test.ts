import { describe, expect, it } from 'vitest';
import { arrowKey, buildGeometry, dofVector, nodeColors, rigidShape, type MimoGeometry } from './geometry';
import { lightness } from './scales';

const cube: MimoGeometry = {
	sensors: [
		{ label: 'A1', position: [0.5, 0.5, 0], direction: [0, 0, 1] },
		{ label: 'A2', position: [-0.5, 0.5, 0], direction: [1, 0, 0] },
		{ label: 'A3', position: [0, -0.5, 0], direction: [0, 1, 0] }
	],
	drives: [
		{ label: 'D1', position: [0, 0, -0.5], direction: [0, 0, 1] },
		{ label: 'D2', position: [-0.6, 0, 0], direction: [1, 0, 0] }
	],
	reference_point: [0, 0, 0],
	length_unit: 'm'
};

describe('the geometry view', () => {
	it('places a marker per sensor, outlined, an arrow per drive, and the box around the sensors', () => {
		const m = buildGeometry(cube, [1, 0.5]);
		expect(m.sensorNodes).toEqual([
			[8, 6],
			[14, 6],
			[20, 6]
		]);
		expect(m.driveNodes.length).toBe(2);
		expect(m.view.nodes.length / 3).toBe(8 + 3 * 6 + 2 * 13);
		expect(m.view.triangles!.length / 3).toBe(3 * 8 + 2 * 16);
		expect(m.view.lines!.length / 2).toBe(12 + 3 * 12);
		const n = m.view.nodes.length / 3;
		for (const i of m.view.triangles!) expect(i).toBeLessThan(n);
		// no motion without the six degrees of freedom
		expect(m.view.modes).toEqual([]);
	});

	it('an arrow is longer for a larger drive rms', () => {
		const tip = (rms: number[], k: number) => {
			const m = buildGeometry(cube, rms);
			const first = m.driveNodes[k][0];
			const p = m.view.nodes.subarray(3 * first, 3 * first + 3);
			const b = cube.drives[k].position;
			return Math.hypot(p[0] - b[0], p[1] - b[1], p[2] - b[2]);
		};
		expect(tip([1, 0.2], 0)).toBeGreaterThan(tip([1, 0.2], 1));
		expect(arrowKey([1, 0.52])).toBe(arrowKey([1, 0.49]));
		expect(arrowKey([1, 0.2])).not.toBe(arrowKey([1, 0.5]));
	});

	it('rigid-body motion: translation moves every node alike; rotation about Z moves +x toward +y', () => {
		const nodes = Float32Array.from([1, 0, 0, 0, 1, 0]);
		const t = rigidShape(nodes, [0, 0, 0], [0.1, 0, 0, 0, 0, 0]);
		expect([...t]).toEqual([0.1, 0, 0, 0.1, 0, 0].map((v) => Math.fround(v)));
		const r = rigidShape(nodes, [0, 0, 0], [0, 0, 0, 0, 0, 1]);
		expect([...r]).toEqual([0, 1, 0, -1, 0, 0]);
		const m = buildGeometry(cube, [1, 1], [0, 0, 1, 0, 0, 0]);
		expect(m.view.modes.length).toBe(1);
	});

	it('reads the six degrees of freedom by name, in order', () => {
		const rows = ['RZ', 'X', 'Y', 'z', 'RX', 'RY'].map((label, i) => ({ label, value: i + 1 }));
		expect(dofVector(rows)).toEqual([2, 3, 4, 5, 6, 1]);
		expect(dofVector(rows.slice(1))).toBeNull();
	});

	it('colors each sensor by its error on the lightness-ordered scale, everything else neutral', () => {
		const m = buildGeometry(cube, [1, 1]);
		const c = nodeColors(m, [0, 4, 9], [], 'light', [0.5, 0.5, 0.5]);
		const L = (node: number) => lightness([c[3 * node] * 255, c[3 * node + 1] * 255, c[3 * node + 2] * 255]);
		const [s0, s1, s2] = m.sensorNodes.map(([first]) => L(first));
		expect(s0).toBeGreaterThan(s1);
		expect(s1).toBeGreaterThan(s2);
		expect(L(0)).toBeCloseTo(L(m.driveNodes[0][0]), 6);
	});
});
