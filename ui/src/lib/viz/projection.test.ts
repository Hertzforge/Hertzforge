import { describe, expect, it } from 'vitest';
import { Box3, Raycaster, Vector2, Vector3 } from 'three';
import { SQUARE, StageCamera, isSquare, lookAtFace, turnView } from './projection';
import { TrackballControls } from './trackball';
import { fitDistance } from './fit';
import { CUBE_FACES, TURN_ARC, cubeFaceUnder, cubeLetters, triadScene, turnArrowPaths, turnArrowUnder } from './arrows';

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

describe('the quarter-turn arrows over the view cube', () => {
	const target = new Vector3(1, 2, 3);
	const facing = (axis: number, sign: number) => {
		const cam = { position: new Vector3(4, -2, 5), up: new Vector3(0, 0, 1) };
		lookAtFace(cam, target, axis, sign);
		return cam;
	};
	const close = (a: Vector3, b: Vector3) => a.toArray().forEach((v, c) => expect(v).toBeCloseTo(b.getComponent(c), 12));

	it('turns the picture clockwise about the line of sight from every face, the camera staying put', () => {
		const view = new StageCamera(32, 1, 0.1, 100);
		for (const { axis, sign } of CUBE_FACES) {
			const cam = facing(axis, sign);
			const position = cam.position.clone();
			const up = cam.up.clone();
			const sight = target.clone().sub(position).normalize();
			turnView(cam, target, 1);
			close(cam.position, position);
			expect(cam.up.length()).toBeCloseTo(1, 12);
			expect(cam.up.dot(sight)).toBeCloseTo(0, 12);
			expect(cam.up.dot(up)).toBeCloseTo(0, 12);
			// the old up and the new one turn about the line of sight, clockwise as seen along it
			close(up.clone().cross(cam.up), sight.clone().negate());
			// what was up on screen is now to the right of the middle
			view.position.copy(cam.position);
			view.up.copy(cam.up);
			view.lookAt(target);
			view.lookedAt.copy(target);
			view.updateMatrixWorld();
			view.updateProjectionMatrix();
			const was = target.clone().add(up).project(view);
			expect(was.x).toBeGreaterThan(0.01);
			expect(was.y).toBeCloseTo(0, 9);
		}
	});

	it('turns a view square within the tolerance, its up left as a drag leaves it, about its own line of sight', () => {
		// a degree off looking along -Y, with Z kept as up though it is no longer square to the sight
		const a = Math.PI / 180;
		const cam = { position: target.clone().add(new Vector3(Math.sin(a), 6 * Math.cos(a), 0)), up: new Vector3(0.02, 0, 1).normalize() };
		expect(isSquare(cam.position, target)).toBe(true);
		const sight = target.clone().sub(cam.position).normalize();
		const position = cam.position.clone();
		turnView(cam, target, 1);
		close(cam.position, position);
		expect(cam.up.length()).toBeCloseTo(1, 12);
		expect(cam.up.dot(sight)).toBeCloseTo(0, 12);
		turnView(cam, target, 1);
		turnView(cam, target, 1);
		turnView(cam, target, 1);
		expect(cam.up.dot(sight)).toBeCloseTo(0, 12);
	});

	it('puts Y on the right looking down from above', () => {
		const cam = facing(2, 1);
		turnView(cam, target, 1);
		close(cam.up, new Vector3(-1, 0, 0));
		turnView(cam, target, -1);
		turnView(cam, target, -1);
		close(cam.up, new Vector3(1, 0, 0));
	});

	it('comes back after four clockwise turns, and after one each way', () => {
		for (const { axis, sign } of CUBE_FACES) {
			const cam = facing(axis, sign);
			const up = cam.up.clone();
			for (let k = 0; k < 4; k++) {
				turnView(cam, target, 1);
				if (k < 3) expect(cam.up.distanceTo(up)).toBeGreaterThan(1);
			}
			close(cam.up, up);
			turnView(cam, target, 1);
			turnView(cam, target, -1);
			close(cam.up, up);
			turnView(cam, target, -1);
			turnView(cam, target, 1);
			close(cam.up, up);
		}
	});

	it('shows while the view is square to an axis, and not off it', () => {
		for (const { axis, sign } of CUBE_FACES) {
			const cam = facing(axis, sign);
			expect(isSquare(cam.position, target)).toBe(true);
			turnView(cam, target, 1);
			expect(isSquare(cam.position, target)).toBe(true);
		}
		expect(isSquare(new Vector3(4, -2, 5), target)).toBe(false);
		// tipped from straight down by less than the tolerance, and by more
		const tipped = (degrees: number) => {
			const a = (degrees * Math.PI) / 180;
			return target.clone().add(new Vector3(Math.sin(a), 0, Math.cos(a)).multiplyScalar(7));
		};
		expect(Math.cos((2 * Math.PI) / 180)).toBeGreaterThan(SQUARE);
		expect(isSquare(tipped(2), target)).toBe(true);
		expect(isSquare(tipped(3), target)).toBe(false);
		expect(isSquare(target.clone(), target)).toBe(false);
	});

	it('takes a press on either arrow where it is drawn, and none on the cube, the axes or off the marker', () => {
		const box = { left: 100, top: 200, width: 72, height: 72 };
		const pixel = (u: number, v: number): [number, number] => [box.left + (u / 48) * box.width, box.top + (v / 48) * box.height];
		for (const turn of [1, -1] as const) {
			const { band, head } = turnArrowPaths(turn);
			// every point the arrow is drawn through: the arc's ends (past the radii and flags) and the head's corners
			const numbers = (d: string) => (d.match(/-?\d+(\.\d+)?/g) ?? []).map(Number);
			const b = numbers(band);
			const points = [[b[0], b[1]], [b[7], b[8]]];
			const h = numbers(head);
			for (let k = 0; k < h.length; k += 2) points.push([h[k], h[k + 1]]);
			expect(points).toHaveLength(5);
			for (const [u, v] of points) {
				expect(Math.sign(u - 24)).toBe(turn);
				expect(v).toBeLessThan(24);
				expect(turnArrowUnder(box, ...pixel(u, v))).toBe(turn);
			}
			const middle = ((TURN_ARC.from + TURN_ARC.to) / 2) * (Math.PI / 180);
			expect(turnArrowUnder(box, ...pixel(24 + turn * TURN_ARC.radius * Math.sin(middle), 24 - TURN_ARC.radius * Math.cos(middle)))).toBe(turn);
		}
		expect(turnArrowUnder(box, ...pixel(24, 24))).toBeNull();
		// the axis pointing up, and the one pointing right
		expect(turnArrowUnder(box, ...pixel(24, 24 - TURN_ARC.radius))).toBeNull();
		expect(turnArrowUnder(box, ...pixel(24 + TURN_ARC.radius, 24))).toBeNull();
		expect(turnArrowUnder(box, ...pixel(24, 24 + TURN_ARC.radius))).toBeNull();
		expect(turnArrowUnder(box, ...pixel(1, 1))).toBeNull();
		expect(turnArrowUnder(box, 99, 236)).toBeNull();
		expect(turnArrowUnder({ left: 0, top: 0, width: 0, height: 0 }, 0, 0)).toBeNull();
	});

	it('sits clear of the cube from every face: no press is both an arrow\'s and a face\'s', () => {
		const triad = triadScene();
		const view = new StageCamera(32, 1, 0.1, 100);
		const box = { left: 100, top: 200, width: 72, height: 72 };
		for (const { axis, sign } of CUBE_FACES) {
			const cam = facing(axis, sign);
			for (let turn = 0; turn < 4; turn++) {
				view.position.copy(cam.position);
				view.up.copy(cam.up);
				view.lookAt(target);
				let arrows = 0;
				for (let x = box.left; x <= box.left + box.width; x += 0.5) {
					for (let y = box.top; y <= box.top + box.height / 2; y += 0.5) {
						if (turnArrowUnder(box, x, y) === null) continue;
						arrows++;
						expect(cubeFaceUnder(triad, box, view.quaternion, x, y)).toBeNull();
					}
				}
				expect(arrows).toBeGreaterThan(100);
				turnView(cam, target, 1);
			}
		}
		triad.dispose();
	});
});
