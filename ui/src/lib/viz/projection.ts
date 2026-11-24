/**
 * The camera of a 3-D view, in parallel projection: lines that are parallel on the model are
 * parallel on screen, and a length reads the same at any depth, so a mode's deflection at the
 * back of a model, a far bar of a MAC chart or a far curve of a waterfall is drawn the size of
 * the same one in front. Perspective was offered too, and removed: on these views it made equal
 * values look unequal, and the report's scene was parallel only.
 *
 * The picture is as tall as a perspective camera of the same field of view would show at
 * `lookedAt`, the point the camera looks at, so zooming moves the camera toward that point and a
 * camera kept by its position, that point and its up direction comes back as it was.
 */
import { PerspectiveCamera, Vector3 } from 'three';

export class StageCamera extends PerspectiveCamera {
	/** The point the camera looks at: the controls' target. */
	lookedAt = new Vector3();

	constructor(fov?: number, aspect?: number, near?: number, far?: number) {
		super(fov, aspect, near, far);
		// three asks a camera which kind it is, to aim a ray through a pixel and to light a surface
		const kind = this as unknown as { isPerspectiveCamera: boolean; isOrthographicCamera: boolean };
		kind.isPerspectiveCamera = false;
		kind.isOrthographicCamera = true;
		this.updateProjectionMatrix();
	}

	/** Half the height of what the view shows. */
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

	override updateProjectionMatrix(): void {
		// three's constructor calls this before `lookedAt` is set
		if (!this.lookedAt) return super.updateProjectionMatrix();
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

/** Whether a camera at `position` looks squarely along one of the axes at `target`: the views a face of the cube turns to. */
export function isSquare(position: Vector3, target: Vector3): boolean {
	const away = position.clone().sub(target);
	const distance = away.length();
	if (!distance) return false;
	return Math.max(Math.abs(away.x), Math.abs(away.y), Math.abs(away.z)) / distance > SQUARE;
}

/**
 * Turn the view a quarter turn about the line of sight: the picture turns clockwise on screen for
 * `turn` 1 and counterclockwise for -1. The camera stays where it is and keeps looking at
 * `target`; only which way is up changes.
 */
export function turnView(camera: { position: Vector3; up: Vector3 }, target: Vector3, turn: 1 | -1): void {
	const sight = target.clone().sub(camera.position).normalize();
	// the up a camera is drawn with is its own up made square to the line of sight
	const up = camera.up.clone().addScaledVector(sight, -camera.up.dot(sight)).normalize();
	camera.up.copy(sight.cross(up).multiplyScalar(-turn));
}
