import { describe, expect, it } from 'vitest';
import { Box3, Raycaster, Vector2, Vector3 } from 'three';
import { StageCamera, lookAtFace } from './projection';
import { TrackballControls } from './trackball';
import { fitDistance } from './fit';
import { CUBE_FACES, cubeFaceUnder, cubeLetters, triadScene } from './arrows';

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

describe('a 3-D view in parallel projection', () => {
	it('says it is orthographic, as three asks a camera', () => {
		const { cam } = camera();
		expect((cam as unknown as { isOrthographicCamera: boolean }).isOrthographicCamera).toBe(true);
		expect(cam.isPerspectiveCamera).toBe(false);
	});

	it('draws a length the same at any depth', () => {
		const { cam } = camera([0, -10, 0], [0, 0, 0]);
		const width = (y: number) => drawn(cam, [1, y, 0])[0] - drawn(cam, [-1, y, 0])[0];
		expect(width(3)).toBeCloseTo(width(-3), 12);
		expect(width(3)).toBeCloseTo(width(0), 12);
	});

	it('is as tall at what it looks at as a perspective camera of the same field', () => {
		const { cam } = camera([0, -10, 0], [0, 0, 0]);
		// tan(16 degrees) * 10 is half the height shown
		expect(drawn(cam, [0, 0, 10 * Math.tan((16 * Math.PI) / 180)])[1]).toBeCloseTo(1, 9);
	});

	it('zooms by the camera\'s distance', () => {
		const { cam, target } = camera([0, -10, 0], [0, 0, 0]);
		const controls = { target, update: TrackballControls.prototype.update, camera: cam, moved: false };
		const wide = drawn(cam, [1, 0, 0])[0];
		cam.position.set(0, -5, 0);
		controls.update();
		expect(drawn(cam, [1, 0, 0])[0]).toBeCloseTo(2 * wide, 9);
	});

	it('comes back as it was from a kept position, target and up direction', () => {
		const { cam, target } = camera();
		const kept = { position: cam.position.toArray(), target: target.toArray(), up: cam.up.toArray() };
		const before = drawn(cam, [1, 1, 1]);
		// a redraw makes the camera again and puts it where it was
		const { cam: again } = camera(kept.position, kept.target);
		again.up.fromArray(kept.up);
		drawn(again, [1, 1, 1]).forEach((v, c) => expect(v).toBeCloseTo(before[c], 9));
	});

	it('still draws what is behind the camera zoomed far in', () => {
		const { cam } = camera([0, -0.1, 0], [0, 0, 0]);
		cam.updateMatrixWorld();
		const behind = new Vector3(0, -5, 0).project(cam);
		expect(behind.z).toBeGreaterThan(-1);
		expect(behind.z).toBeLessThan(1);
	});

	it('aims a ray straight along the line of sight through any pixel', () => {
		const { cam } = camera([0, -10, 0], [0, 0, 0]);
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

	it('fits a box by its width and height alone, its depth not counting', () => {
		const box = new Box3(new Vector3(-1, -4, -0.5), new Vector3(1, 4, 0.5));
		const deep = new Box3(new Vector3(-1, -8, -0.5), new Vector3(1, 8, 0.5));
		const centre = new Vector3();
		const dir = new Vector3(0, -1, 0);
		const up = new Vector3(0, 0, 1);
		const distance = fitDistance(box, centre, dir, up, 32, 1.5);
		expect(fitDistance(deep, centre, dir, up, 32, 1.5)).toBeCloseTo(distance, 12);
		const { cam } = camera([0, -distance, 0], [0, 0, 0]);
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

	it('names the face under a press inside the marker, and nothing outside it', () => {
		const triad = triadScene();
		const view = new StageCamera(32, 1, 0.1, 100);
		const box = { left: 100, top: 200, right: 164, bottom: 264, width: 64, height: 64 };
		for (const { axis, sign } of CUBE_FACES) {
			const cam = looking([4, -2, 5]);
			lookAtFace(cam, target, axis, sign);
			view.position.copy(cam.position);
			view.up.copy(cam.up);
			view.lookAt(target);
			expect(cubeFaceUnder(triad, box, view.quaternion, 132, 232)).toEqual({ axis, sign });
			expect(cubeFaceUnder(triad, box, view.quaternion, 99, 232)).toBeNull();
			expect(cubeFaceUnder(triad, box, view.quaternion, 132, 265)).toBeNull();
		}
		triad.dispose();
	});

	it('writes X to the right and Y up when looking down from above with Y up', () => {
		const view = new StageCamera(32, 1, 0.1, 100);
		view.position.set(0, 0, 5);
		view.up.set(0, 1, 0);
		view.lookAt(0, 0, 0);
		const [x, y, z] = cubeLetters(view.quaternion);
		expect(x.x).toBeGreaterThan(40);
		expect(x.y).toBeCloseTo(27, 9);
		expect(y.x).toBeCloseTo(24, 9);
		expect(y.y).toBeLessThan(10);
		// Z points at the eye, so its letter sits on the middle
		expect(z.x).toBeCloseTo(24, 9);
		expect(z.y).toBeCloseTo(27, 9);
	});

	it('paints the negative faces paler than the positive ones of their axes', () => {
		const triad = triadScene();
		const cube = triad.scene.children.find((o) => Array.isArray((o as unknown as { material: unknown }).material)) as unknown as { material: { color: { r: number; g: number; b: number } }[] };
		const light = (k: number) => cube.material[k].color.r + cube.material[k].color.g + cube.material[k].color.b;
		for (const k of [0, 2, 4]) expect(light(k + 1)).toBeGreaterThan(light(k));
		triad.dispose();
	});
});
