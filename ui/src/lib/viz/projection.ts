/**
 * The camera of a 3-D view, drawn in perspective or in parallel projection, where lines that are
 * parallel on the model are parallel on screen and a length reads the same at any depth.
 *
 * One camera does both. In parallel projection the picture is as tall as the perspective one is
 * at `lookedAt`, the point the camera looks at, so the scene keeps its size on screen across the
 * switch, zooming by moving the camera works the same in either, and a camera kept by its
 * position, the point it looks at and its up direction comes back as it was in either projection.
 */
import { PerspectiveCamera, Vector3 } from 'three';

export class StageCamera extends PerspectiveCamera {
	/** Drawn in parallel projection. Set with `setParallel`. */
	parallel = false;
	/** The point the camera looks at: the controls' target. */
	lookedAt = new Vector3();

	/** Half the height of what a parallel view shows: the perspective view's at the point looked at. */
	get span(): number {
		const distance = Math.max(this.position.distanceTo(this.lookedAt), 1e-9);
		return (distance * Math.tan((this.fov * Math.PI) / 360)) / this.zoom;
	}

	// what three's own orthographic camera has, read by the handles of a gizmo to size themselves
	get top(): number {
		return this.span;
	}
	get bottom(): number {
		return -this.span;
	}

	setParallel(parallel: boolean): void {
		this.parallel = parallel;
		// three asks a camera which kind it is, to aim a ray through a pixel and to light a surface
		const kind = this as unknown as { isPerspectiveCamera: boolean; isOrthographicCamera: boolean };
		kind.isPerspectiveCamera = !parallel;
		kind.isOrthographicCamera = parallel;
		this.updateProjectionMatrix();
	}

	override updateProjectionMatrix(): void {
		if (!this.parallel) return super.updateProjectionMatrix();
		const half = this.span;
		const wide = half * this.aspect;
		// a parallel view has no eye for the model to pass behind: what is behind the camera, as
		// when zoomed far in, is drawn too
		const depth = Math.max(this.far, 2 * this.position.distanceTo(this.lookedAt));
		this.projectionMatrix.makeOrthographic(-wide, wide, half, -half, -depth, depth, this.coordinateSystem);
		this.projectionMatrixInverse.copy(this.projectionMatrix).invert();
	}
}

/**
 * Turn a camera to look straight at a face of the model's axes: from the +X side for axis 0 and
 * sign 1, from below for axis 2 and sign -1. Looking at that face already, it turns to the
 * opposite one. What it looks at and how far it stands stay as they are. Z is up on screen from
 * the sides; from above Y is up, and from below it is down. Returns the face now looked at.
 */
export function lookAtFace(camera: { position: Vector3; up: Vector3 }, target: Vector3, axis: number, sign: number): { axis: number; sign: number } {
	const away = camera.position.clone().sub(target);
	const distance = away.length() || 1;
	const toward = new Vector3().setComponent(axis, sign);
	if (away.dot(toward) / distance > SQUARE) {
		sign = -sign;
		toward.negate();
	}
	camera.position.copy(target).addScaledVector(toward, distance);
	if (axis === 2) camera.up.set(0, sign, 0);
	else camera.up.set(0, 0, 1);
	return { axis, sign };
}

/** Looking this squarely at a face already, a click on it turns the view to the opposite face. */
export const SQUARE = 0.999;
