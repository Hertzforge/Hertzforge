import { describe, expect, it } from 'vitest';
import { Box3, Vector3 } from 'three';
import { FILL, fitDistance, homeFit, homePose } from './fit';
import { StageCamera } from './projection';

/** The box's corners on screen, as fractions of the half-width and half-height. */
function onScreen(box: Box3, dir: Vector3, up: Vector3, aspect: number) {
	const centre = box.getCenter(new Vector3());
	const camera = new StageCamera(32, aspect, 0.001, 1000);
	camera.position.copy(centre).addScaledVector(dir, fitDistance(box, centre, dir, up, 32, aspect));
	camera.up.copy(up);
	camera.lookedAt.copy(centre);
	camera.lookAt(centre);
	camera.updateMatrixWorld();
	camera.updateProjectionMatrix();
	let x = 0;
	let y = 0;
	for (let k = 0; k < 8; k++) {
		const p = new Vector3(k & 1 ? box.max.x : box.min.x, k & 2 ? box.max.y : box.min.y, k & 4 ? box.max.z : box.min.z).project(camera);
		x = Math.max(x, Math.abs(p.x));
		y = Math.max(y, Math.abs(p.y));
	}
	return { x, y };
}

describe('framing a model', () => {
	// the fridge: 1.76 m tall along Y, seen as its geometry's view says
	const fridge = new Box3(new Vector3(-0.4, 0, -0.35), new Vector3(0.4, 1.76, 0.35));
	const { dir, up } = homePose({ eye: [1, 0.55, 1.2], up: [0, 1, 0] });

	it('keeps a tall model inside a wide pane, filling its height', () => {
		const { x, y } = onScreen(fridge, dir, up, 16 / 9);
		expect(y).toBeLessThanOrEqual(FILL + 1e-6);
		expect(x).toBeLessThan(y);
		expect(y).toBeGreaterThan(FILL * 0.85);
	});

	it('keeps a long model inside a tall pane, filling its width', () => {
		const beam = new Box3(new Vector3(-3, -0.1, -0.1), new Vector3(3, 0.1, 0.1));
		const front = homePose();
		const { x, y } = onScreen(beam, front.dir, front.up, 9 / 16);
		expect(x).toBeLessThanOrEqual(FILL + 1e-6);
		expect(x).toBeGreaterThan(y);
	});

	it("opens at the geometry's own view, up square to the line of sight", () => {
		expect(up.dot(dir)).toBeCloseTo(0, 9);
		expect(up.y).toBeGreaterThan(0.9);
		const plain = homePose();
		expect(plain.up.z).toBeGreaterThan(0.9);
		expect(plain.dir.y).toBeLessThan(0);
	});
});

describe('framing part of a model', () => {
	// a 10 in. plate with a 5 x 2 x 1 in. component bolted to its middle
	const plate = new Box3(new Vector3(-5, -5, -0.25), new Vector3(5, 5, 1.25));
	const part = { lo: [-2.5, -1, 0.25], hi: [2.5, 1, 1.25] };
	const { dir, up } = homePose();

	it('looks at the middle of the part and fits the part, not the whole model', () => {
		const centre = plate.getCenter(new Vector3());
		const framed = homeFit({ frame: part }, plate, centre, dir, up, 32, 16 / 9);
		expect(framed.target.toArray()).toEqual([0, 0, 0.75]);
		const box = new Box3(new Vector3().fromArray(part.lo), new Vector3().fromArray(part.hi));
		expect(framed.distance).toBeCloseTo(fitDistance(box, framed.target, dir, up, 32, 16 / 9), 9);
		const whole = homeFit(undefined, plate, centre, dir, up, 32, 16 / 9);
		expect(whole.target.toArray()).toEqual(centre.toArray());
		expect(whole.distance).toBeCloseTo(fitDistance(plate, centre, dir, up, 32, 16 / 9), 9);
		// the part fills the frame: the camera stands well inside the whole model's distance
		expect(framed.distance).toBeLessThan(0.6 * whole.distance);
	});

	it('opens from the front right when the frame comes with no eye, and from the eye when it does', () => {
		const plain = homePose({ frame: part });
		expect(plain.dir.toArray()).toEqual(homePose().dir.toArray());
		expect(plain.up.toArray()).toEqual(homePose().up.toArray());
		const top = homePose({ eye: [0, 0, 1], up: [0, 1, 0], frame: part });
		expect(top.dir.toArray()).toEqual([0, 0, 1]);
		expect(top.up.toArray()).toEqual([0, 1, 0]);
	});
});
