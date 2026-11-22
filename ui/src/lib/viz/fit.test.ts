import { describe, expect, it } from 'vitest';
import { Box3, Vector3 } from 'three';
import { FILL, fitDistance, homePose } from './fit';
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
