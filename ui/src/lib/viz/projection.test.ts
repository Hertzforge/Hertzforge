import { describe, expect, it } from 'vitest';
import { Box3, Raycaster, Vector2, Vector3 } from 'three';
import { StageCamera, lookAtFace } from './projection';
import { TrackballControls } from './trackball';
import { fitDistance } from './fit';
import { CUBE_FACES, triadScene } from './arrows';

function camera(position = [3, -4, 2], target = [0.5, 0.25, 0]): { cam: StageCamera; target: Vector3 } {
	const cam = new StageCamera(32, 1.5, 0.01, 100);
	const at = new Vector3().fromArray(target);
	cam.position.fromArray(position);
	cam.up.set(0, 0, 1);
	cam.lookedAt.copy(at);
	cam.lookAt(at);
	cam.updateMatrixWorld();
	cam.updateProjectionMatrix();
	return { cam, target: at };
}

/** Where a point of the model lands in the picture, x and y from -1 to 1. */
const drawn = (cam: StageCamera, point: number[]) => {
	cam.updateMatrixWorld();
	const p = new Vector3().fromArray(point).project(cam);
	return [p.x, p.y];
};

describe('a 3-D view in perspective or parallel projection', () => {
	it('opens in perspective and says which it is, as three asks a camera', () => {
		const { cam } = camera();
		expect(cam.parallel).toBe(false);
		expect(cam.isPerspectiveCamera).toBe(true);
		cam.setParallel(true);
		expect((cam as unknown as { isOrthographicCamera: boolean }).isOrthographicCamera).toBe(true);
		expect(cam.isPerspectiveCamera).toBe(false);
		cam.setParallel(false);
		expect(cam.isPerspectiveCamera).toBe(true);
	});

	it('keeps the scene its size on screen across the switch, at what the camera looks at', () => {
		const { cam, target } = camera();
		// points in the plane through the target, square to the line of sight
		const right = new Vector3().setFromMatrixColumn(cam.matrixWorld, 0);
		const up = new Vector3().setFromMatrixColumn(cam.matrixWorld, 1);
		const points = [target.clone().addScaledVector(right, 0.7), target.clone().addScaledVector(up, -0.4), target.clone()].map((p) => p.toArray());
		const before = points.map((p) => drawn(cam, p));
		cam.setParallel(true);
		points.forEach((p, k) => drawn(cam, p).forEach((v, c) => expect(v).toBeCloseTo(before[k][c], 9)));
		cam.setParallel(false);
		points.forEach((p, k) => drawn(cam, p).forEach((v, c) => expect(v).toBeCloseTo(before[k][c], 9)));
	});

	it('draws a length the same at any depth in parallel projection, and smaller farther off in perspective', () => {
		const { cam } = camera([0, -10, 0], [0, 0, 0]);
		const width = (y: number) => drawn(cam, [1, y, 0])[0] - drawn(cam, [-1, y, 0])[0];
		expect(width(3)).toBeLessThan(width(-3));
		cam.setParallel(true);
		expect(width(3)).toBeCloseTo(width(-3), 12);
		expect(width(3)).toBeCloseTo(width(0), 12);
	});

	it('zooms a parallel view by the camera\'s distance, as a perspective one', () => {
		const { cam, target } = camera([0, -10, 0], [0, 0, 0]);
		cam.setParallel(true);
		const controls = { target, update: TrackballControls.prototype.update, camera: cam, moved: false };
		const wide = drawn(cam, [1, 0, 0])[0];
		cam.position.set(0, -5, 0);
		controls.update();
		expect(drawn(cam, [1, 0, 0])[0]).toBeCloseTo(2 * wide, 9);
	});

	it('comes back as it was from a kept position, target and up direction, in either projection', () => {
		for (const parallel of [false, true]) {
			const { cam, target } = camera();
			cam.setParallel(parallel);
			const kept = { position: cam.position.toArray(), target: target.toArray(), up: cam.up.toArray() };
			const before = drawn(cam, [1, 1, 1]);
			// a redraw makes the camera again and puts it where it was
			const { cam: again } = camera(kept.position, kept.target);
			again.up.fromArray(kept.up);
			again.setParallel(parallel);
			drawn(again, [1, 1, 1]).forEach((v, c) => expect(v).toBeCloseTo(before[c], 9));
		}
	});

	it('still draws what is behind the camera of a parallel view zoomed far in', () => {
		const { cam } = camera([0, -0.1, 0], [0, 0, 0]);
		cam.setParallel(true);
		cam.updateMatrixWorld();
		const behind = new Vector3(0, -5, 0).project(cam);
		expect(behind.z).toBeGreaterThan(-1);
		expect(behind.z).toBeLessThan(1);
	});

	it('aims a ray straight along the line of sight through any pixel of a parallel view', () => {
		const { cam } = camera([0, -10, 0], [0, 0, 0]);
		cam.setParallel(true);
		cam.updateMatrixWorld();
		const ray = new Raycaster();
		ray.setFromCamera(new Vector2(0.6, -0.3), cam);
		ray.ray.direction.toArray().forEach((v, c) => expect(v).toBeCloseTo([0, 1, 0][c], 9));
		// and through the point drawn at that pixel
		const on = ray.ray.at(ray.ray.origin.distanceTo(new Vector3(ray.ray.origin.x, 0, ray.ray.origin.z)), new Vector3());
		const [x, y] = drawn(cam, on.toArray());
		expect(x).toBeCloseTo(0.6, 9);
		expect(y).toBeCloseTo(-0.3, 9);
	});

	it('fits a box to a parallel view by its width and height alone', () => {
		const box = new Box3(new Vector3(-1, -4, -0.5), new Vector3(1, 4, 0.5));
		const centre = new Vector3();
		const dir = new Vector3(0, -1, 0);
		const up = new Vector3(0, 0, 1);
		const perspective = fitDistance(box, centre, dir, up, 32, 1.5);
		const parallel = fitDistance(box, centre, dir, up, 32, 1.5, true);
		// in perspective the near face, 4 nearer, has to fit; in parallel its depth does not count
		expect(perspective - parallel).toBeCloseTo(4, 9);
		const { cam } = camera([0, -parallel, 0], [0, 0, 0]);
		cam.setParallel(true);
		expect(Math.abs(drawn(cam, [1, -4, 0.5])[0])).toBeLessThanOrEqual(0.9 + 1e-9);
		expect(Math.max(Math.abs(drawn(cam, [1, -4, 0.5])[0]), Math.abs(drawn(cam, [1, -4, 0.5])[1]))).toBeCloseTo(0.9, 9);
	});
});

describe('the view cube', () => {
	const looking = (position: number[], up = [0, 0, 1]) => ({ position: new Vector3().fromArray(position), up: new Vector3().fromArray(up) });
	const target = new Vector3(1, 2, 3);

	it('looks straight at the face clicked, from that face\'s side, keeping the target and the distance', () => {
		for (const { axis, sign } of CUBE_FACES) {
			const cam = looking([4, -2, 5]);
			const distance = cam.position.distanceTo(target);
			expect(lookAtFace(cam, target, axis, sign)).toEqual({ axis, sign });
			const away = cam.position.clone().sub(target);
			expect(away.length()).toBeCloseTo(distance, 12);
			const expected = [0, 0, 0];
			expected[axis] = sign * distance;
			away.toArray().forEach((v, c) => expect(v).toBeCloseTo(expected[c], 12));
		}
	});

	it('has Z up from the sides, Y up from above and Y down from below', () => {
		const up = (axis: number, sign: number) => {
			const cam = looking([4, -2, 5]);
			lookAtFace(cam, target, axis, sign);
			return cam.up.toArray();
		};
		for (const [axis, sign] of [[0, 1], [0, -1], [1, 1], [1, -1]]) expect(up(axis, sign)).toEqual([0, 0, 1]);
		expect(up(2, 1)).toEqual([0, 1, 0]);
		expect(up(2, -1)).toEqual([0, -1, 0]);
	});

	it('turns to the opposite face when the face clicked is the one already faced', () => {
		const cam = looking([4, -2, 5]);
		expect(lookAtFace(cam, target, 0, 1)).toEqual({ axis: 0, sign: 1 });
		expect(lookAtFace(cam, target, 0, 1)).toEqual({ axis: 0, sign: -1 });
		expect(cam.position.x).toBeLessThan(target.x);
		expect(lookAtFace(cam, target, 0, -1)).toEqual({ axis: 0, sign: 1 });
		// from below, the face clicked again is the top, with Y up
		lookAtFace(cam, target, 2, -1);
		expect(lookAtFace(cam, target, 2, -1)).toEqual({ axis: 2, sign: 1 });
		expect(cam.up.toArray()).toEqual([0, 1, 0]);
	});

	it('names the face drawn under a click, and nothing off the cube', () => {
		const triad = triadScene();
		const view = new StageCamera(32, 1, 0.1, 100);
		for (const { axis, sign } of CUBE_FACES) {
			// the main camera looking straight at the face: it is drawn at the middle of the picture
			const cam = looking([4, -2, 5]);
			lookAtFace(cam, target, axis, sign);
			view.position.copy(cam.position);
			view.up.copy(cam.up);
			view.lookAt(target);
			triad.look(view.quaternion);
			expect(triad.faceAt(0, 0)).toEqual({ axis, sign });
			expect(triad.faceAt(0.95, 0.95)).toBeNull();
		}
		triad.dispose();
	});

	it('paints the negative faces paler than the positive ones of their axes', () => {
		const triad = triadScene();
		const cube = triad.scene.children.find((o) => Array.isArray((o as unknown as { material: unknown }).material)) as unknown as { material: { color: { r: number; g: number; b: number } }[] };
		const light = (k: number) => cube.material[k].color.r + cube.material[k].color.g + cube.material[k].color.b;
		for (const k of [0, 2, 4]) expect(light(k + 1)).toBeGreaterThan(light(k));
		triad.dispose();
	});
});
