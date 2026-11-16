import { describe, expect, it } from 'vitest';
import { Color, Vector3 } from 'three';
import { arrowGeometry, arrowMatrices, arrowMesh } from './arrows';
import { AXIS_COLORS } from './pick';

describe('solid arrows', () => {
	it('builds one arrow from the origin to +Y, a unit long, with a head wider than its shaft', () => {
		const g = arrowGeometry();
		g.computeBoundingBox();
		const box = g.boundingBox!;
		expect(box.min.y).toBeCloseTo(0);
		expect(box.max.y).toBeCloseTo(1);
		expect(box.max.x).toBeCloseTo(0.1);
		expect(g.getAttribute('normal')).toBeDefined();
	});

	it('puts each arrow at its tail, along its direction, at the one length', () => {
		const [out, into] = arrowMatrices([1, 2, 3, 0, 0, 0.5], [1, 0, 0, 0, 0, -1], 0.5);
		expect(new Vector3(0, 1, 0).applyMatrix4(out).toArray().map((v) => +v.toFixed(6))).toEqual([1.5, 2, 3]);
		// an arrow pointing in starts a length away and ends on its node
		expect(new Vector3(0, 1, 0).applyMatrix4(into).toArray().map((v) => +v.toFixed(6))).toEqual([0, 0, 0]);
	});

	it('colors each arrow by its own axis, as the orientation axes are', () => {
		const mesh = arrowMesh({ tails: [0, 0, 0, 0, 0, 0, 0, 0, 0], directions: [0, 0, 1, 1, 0, 0, 0, 1, 0], axes: [2, 0, 1], length: 1 });
		expect(mesh.count).toBe(3);
		const color = new Color();
		const expected = [AXIS_COLORS[2], AXIS_COLORS[0], AXIS_COLORS[1]];
		expected.forEach((hex, k) => {
			mesh.getColorAt(k, color);
			expect(color.getHex()).toBe(hex);
		});
	});
});
