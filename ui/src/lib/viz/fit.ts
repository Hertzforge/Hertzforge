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
 * `box` fits a `StageCamera` of vertical field `fovDeg` and width-to-height `aspect`. Its picture
 * is as tall as a perspective one would be at `centre`, and a corner nearer the camera is drawn no
 * larger, so the box is fitted by its width and height alone.
 */
export function fitDistance(box: Box3, centre: Vector3, dir: Vector3, up: Vector3, fovDeg: number, aspect: number): number {
	const toward = dir.clone().normalize();
	const right = new Vector3().crossVectors(up, toward).normalize();
	const upright = new Vector3().crossVectors(toward, right);
	const tanV = Math.tan((fovDeg * Math.PI) / 360) * FILL;
	const tanH = tanV * (aspect || 1);
	let distance = Math.max(box.getSize(new Vector3()).length(), 1e-6) * 0.05;
	const corner = new Vector3();
	for (let k = 0; k < 8; k++) {
		corner.set(k & 1 ? box.max.x : box.min.x, k & 2 ? box.max.y : box.min.y, k & 4 ? box.max.z : box.min.z).sub(centre);
		distance = Math.max(distance, Math.abs(corner.dot(right)) / tanH, Math.abs(corner.dot(upright)) / tanV);
	}
	return distance;
}

/** How a view opens (a `shape3d` view's `home`): looking from `eye` with `up` up, fitted to the box `frame` rather than the whole model. */
export interface Home {
	eye?: number[];
	up?: number[];
	frame?: { lo: number[]; hi: number[] };
}

/** The direction a view opens looking from and the way up, `up` made square to it: `home`'s when given, else from the front right with Z up. */
export function homePose(home?: Home): { dir: Vector3; up: Vector3 } {
	const dir = home?.eye ? new Vector3().fromArray(home.eye).normalize() : new Vector3(0.55, -1.2, 0.58).normalize();
	const up = home?.eye && home.up ? new Vector3().fromArray(home.up) : new Vector3(0, 0, 1);
	up.addScaledVector(dir, -up.dot(dir)).normalize();
	return { dir, up };
}

/**
 * Where a view opens and Reset View returns, looking from `dir` with `up` up: the point looked at
 * and how far off the camera stands. A view framed on part of the model (`home.frame`) looks at
 * the middle of that box and fits it; the rest of the model is still drawn, running out of the
 * frame. Otherwise it looks at `centre` and fits `box`, the whole model.
 */
export function homeFit(home: Home | undefined, box: Box3, centre: Vector3, dir: Vector3, up: Vector3, fovDeg: number, aspect: number): { target: Vector3; distance: number } {
	const framed = home?.frame ? new Box3(new Vector3().fromArray(home.frame.lo), new Vector3().fromArray(home.frame.hi)) : null;
	const target = framed ? framed.getCenter(new Vector3()) : centre.clone();
	return { target, distance: fitDistance(framed ?? box, target, dir, up, fovDeg, aspect) };
}
