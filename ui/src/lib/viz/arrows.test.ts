import { describe, expect, it } from 'vitest';
import { Color, Vector3 } from 'three';
import { Matrix4 } from 'three';
import { arrowGeometry, arrowMatrices, arrowMesh, labelPlaces, labelPoints, moveArrows, riddenOffsets } from './arrows';
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

	it('draws each arrow at its own length when the view gives one each', () => {
		// a picture of a test's forces and responses sizes each set to its own spacing
		const mesh = arrowMesh({ tails: [0, 0, 0, 0, 0, 1], directions: [1, 0, 0, 0, 0, -1], axes: [0, 2], length: 3, lengths: new Float32Array([3, 0.5]) });
		const head = (k: number) => {
			const m = new Matrix4();
			mesh.getMatrixAt(k, m);
			return new Vector3(0, 1, 0).applyMatrix4(m).toArray().map((v) => +v.toFixed(6));
		};
		expect(head(0)).toEqual([3, 0, 0]);
		expect(head(1)).toEqual([0, 0, 0.5]);
		const at = labelPoints({ tails: [0, 0, 0, 0, 0, 1], directions: [1, 0, 0, 0, 0, -1], inward: [0, 1], length: 3, lengths: [3, 0.5] }, 0.2);
		expect(Array.from(at).map((v) => +v.toFixed(6))).toEqual([3.6, 0, 0, 0, 0, 1.1]);
	});

	it('places each label on the picture, leaving out one behind the camera or off the edge', () => {
		// a camera whose device coordinates are the points' own x and y, and z for depth
		const project = (x: number, y: number, z: number): [number, number, number] => [x, y, z];
		const at = [0, 0, 0, 1, 1, 0, 0.5, -0.5, 2, 1.5, 0, 0];
		const places = labelPlaces(at, ['middle', 'corner', 'behind', 'off'], project, 200, 100);
		expect(places).toEqual([
			{ x: 100, y: 50, text: 'middle' },
			{ x: 200, y: 0, text: 'corner' }
		]);
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

	it('labels an arrow past its free end: the head of one pointing out, the tail of one pointing in', () => {
		const at = labelPoints({ tails: [0, 0, 0, 0, 0, 2], directions: [1, 0, 0, 0, 0, -1], inward: [0, 1], length: 2 }, 0.25);
		expect(Array.from(at)).toEqual([2.5, 0, 0, 0, 0, 2.5]);
	});

	it('gives an arrow that follows a node that node\'s offset now, and none to one that stays put', () => {
		const shape = [0, 0, 0, 1, 2, 3];
		const imag = [0, 0, 0, 10, 20, 30];
		const out = riddenOffsets(new Uint32Array([1, 0xffffffff, 7]), shape, imag, 0.5, -0.1, new Float32Array(9));
		expect(Array.from(out).map((v) => +v.toFixed(6))).toEqual([-0.5, -1, -1.5, 0, 0, 0, 0, 0, 0]);
	});

	it('moves each arrow from where it rests, and back when its node is back', () => {
		const tails = [0, 0, 1, 2, 0, 0];
		const directions = [0, 0, -1, 1, 0, 0];
		const mesh = arrowMesh({ tails, directions, axes: [2, 0], length: 1 });
		const rest = arrowMatrices(tails, directions, 1);
		const head = (k: number) => {
			const m = new Matrix4();
			mesh.getMatrixAt(k, m);
			return new Vector3(0, 1, 0).applyMatrix4(m).toArray().map((v) => +v.toFixed(6));
		};
		moveArrows(mesh, rest, [0, 0, 0.25, 0, 0, 0]);
		expect(head(0)).toEqual([0, 0, 0.25]);
		expect(head(1)).toEqual([3, 0, 0]);
		moveArrows(mesh, rest, [0, 0, -0.25, 0, 0, 0]);
		expect(head(0)).toEqual([0, 0, -0.25]);
		moveArrows(mesh, rest, [0, 0, 0, 0, 0, 0]);
		expect(head(0)).toEqual([0, 0, 0]);
	});
});
