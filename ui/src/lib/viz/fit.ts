/**
 * Framing a model in a 3-D view: how far off the camera stands so the model's box fills the pane.
 * Fitted to what the box projects in both fields of view, so a tall model in a wide pane, or a
 * long one in a tall pane, is not cut off, and a flat one does not open small (Brandon,
 * 2026-11-14: a 1.76 m fridge, Y up, was cut off at the top and bottom of a widescreen video).
 */
import { Box3, Vector3 } from 'three';

/** The share of the pane the model fills in its tighter direction. */
export const FILL = 0.9;

/**
 * The distance from `centre`, looking from the direction `dir` with `up` on screen, at which
 * `box` fits a perspective camera of vertical field `fovDeg` and width-to-height `aspect`. In
 * `parallel` projection the picture is as tall as the perspective one is at `centre`, and a
 * corner nearer the camera is drawn no larger, so the box is fitted by its width and height alone.
 */
export function fitDistance(box: Box3, centre: Vector3, dir: Vector3, up: Vector3, fovDeg: number, aspect: number, parallel = false): number {
	const toward = dir.clone().normalize();
	const right = new Vector3().crossVectors(up, toward).normalize();
	const upright = new Vector3().crossVectors(toward, right);
	const tanV = Math.tan((fovDeg * Math.PI) / 360) * FILL;
	const tanH = tanV * (aspect || 1);
	let distance = Math.max(box.getSize(new Vector3()).length(), 1e-6) * 0.05;
	const corner = new Vector3();
	for (let k = 0; k < 8; k++) {
		corner.set(k & 1 ? box.max.x : box.min.x, k & 2 ? box.max.y : box.min.y, k & 4 ? box.max.z : box.min.z).sub(centre);
		const near = parallel ? 0 : corner.dot(toward);
		distance = Math.max(distance, near + Math.abs(corner.dot(right)) / tanH, near + Math.abs(corner.dot(upright)) / tanV);
	}
	return distance;
}

/** The direction a view opens looking from and the way up, `up` made square to it: `home` when given, else from the front right with Z up. */
export function homePose(home?: { eye: number[]; up: number[] }): { dir: Vector3; up: Vector3 } {
	const dir = home ? new Vector3().fromArray(home.eye).normalize() : new Vector3(0.55, -1.2, 0.58).normalize();
	const up = home ? new Vector3().fromArray(home.up) : new Vector3(0, 0, 1);
	up.addScaledVector(dir, -up.dot(dir)).normalize();
	return { dir, up };
}
