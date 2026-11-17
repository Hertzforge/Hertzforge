/**
 * Solid arrows: a round shaft and a cone head, shaded by the scene's lights. The DOF arrows over a
 * model and the orientation axes in the corner are both made of them, in the axes' colors (X red,
 * Y green, Z blue).
 */
import {
	AmbientLight,
	BoxGeometry,
	BufferGeometry,
	Color,
	ConeGeometry,
	CylinderGeometry,
	DirectionalLight,
	InstancedMesh,
	Matrix4,
	MeshLambertMaterial,
	OrthographicCamera,
	Quaternion,
	Raycaster,
	Scene,
	Vector2,
	Vector3,
	Mesh
} from 'three';
import { mergeGeometries } from 'three/addons/utils/BufferGeometryUtils.js';
import { AXIS_COLORS } from './pick';

/** An arrow's proportions as fractions of its length. */
export const ARROW = { shaft: 0.035, head: 0.3, headRadius: 0.1 } as const;

/** One arrow from the origin to +Y, one unit long. */
export function arrowGeometry(shape: { shaft: number; head: number; headRadius: number } = ARROW): BufferGeometry {
	const shaft = new CylinderGeometry(shape.shaft, shape.shaft, 1 - shape.head, 20, 1, false);
	shaft.translate(0, (1 - shape.head) / 2, 0);
	const head = new ConeGeometry(shape.headRadius, shape.head, 24, 1, false);
	head.translate(0, 1 - shape.head / 2, 0);
	const merged = mergeGeometries([shaft, head]);
	shaft.dispose();
	head.dispose();
	return merged;
}

const UP = new Vector3(0, 1, 0);

/** Where each arrow sits: its tail, the unit vector it points along and the one length, as a matrix per arrow. */
export function arrowMatrices(tails: ArrayLike<number>, directions: ArrayLike<number>, length: number): Matrix4[] {
	const out: Matrix4[] = [];
	const at = new Vector3();
	const along = new Vector3();
	const turn = new Quaternion();
	const size = new Vector3(length, length, length);
	for (let k = 0; k < tails.length / 3; k++) {
		at.set(tails[3 * k], tails[3 * k + 1], tails[3 * k + 2]);
		along.set(directions[3 * k], directions[3 * k + 1], directions[3 * k + 2]).normalize();
		turn.setFromUnitVectors(UP, along);
		out.push(new Matrix4().compose(at, turn, size));
	}
	return out;
}

/** The arrows of a view as one instanced mesh, each in the color of its own axis. */
export function arrowMesh(arrows: { tails: ArrayLike<number>; directions: ArrayLike<number>; axes: ArrayLike<number>; length: number }): InstancedMesh {
	const matrices = arrowMatrices(arrows.tails, arrows.directions, arrows.length);
	const mesh = new InstancedMesh(arrowGeometry(), new MeshLambertMaterial(), matrices.length);
	const color = new Color();
	matrices.forEach((matrix, k) => {
		mesh.setMatrixAt(k, matrix);
		mesh.setColorAt(k, color.setHex(AXIS_COLORS[arrows.axes[k]] ?? 0x808080));
	});
	mesh.instanceMatrix.needsUpdate = true;
	if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
	mesh.frustumCulled = false;
	return mesh;
}

/** Half the width of the orientation axes' picture, in arrow lengths. */
export const TRIAD_HALF = 1.38;

/** The view cube's edge, where the orientation axes' arrows are one long. */
export const CUBE_EDGE = 0.6;
/** How much of a negative face's color is white, to tell it from the positive face of its axis. */
export const CUBE_TINT = 0.6;
/** The cube's faces in the order a box's sides are made: (axis, sign) of each. */
export const CUBE_FACES: readonly { axis: number; sign: number }[] = [
	{ axis: 0, sign: 1 },
	{ axis: 0, sign: -1 },
	{ axis: 1, sign: 1 },
	{ axis: 1, sign: -1 },
	{ axis: 2, sign: 1 },
	{ axis: 2, sign: -1 }
];

/**
 * The orientation axes: three solid arrows from a cube at the origin, with their own light and a
 * camera that only turns. `look` points the camera the way the view's camera looks. The cube's
 * faces are in their axes' colors, the negative ones paler, and `faceAt` names the face drawn at
 * a place in the picture (x and y from -1 to 1, y up), or null off the cube: a click there turns
 * the view to look straight at that face (`lookAtFace`).
 */
export function triadScene(): {
	scene: Scene;
	camera: OrthographicCamera;
	look: (q: Quaternion) => void;
	faceAt: (x: number, y: number) => { axis: number; sign: number } | null;
	dispose: () => void;
} {
	const scene = new Scene();
	const stout = { shaft: 0.07, head: 0.36, headRadius: 0.17 };
	const geometry = arrowGeometry(stout);
	const mesh = arrowMesh({ tails: [0, 0, 0, 0, 0, 0, 0, 0, 0], directions: [1, 0, 0, 0, 1, 0, 0, 0, 1], axes: [0, 1, 2], length: 1 });
	mesh.geometry.dispose();
	mesh.geometry = geometry;
	const white = new Color(0xffffff);
	const sides = CUBE_FACES.map(({ axis, sign }) => new MeshLambertMaterial({ color: new Color(AXIS_COLORS[axis]).lerp(white, sign > 0 ? 0 : CUBE_TINT) }));
	const cube = new Mesh(new BoxGeometry(CUBE_EDGE, CUBE_EDGE, CUBE_EDGE), sides);
	scene.add(mesh, cube);
	const camera = new OrthographicCamera(-TRIAD_HALF, TRIAD_HALF, TRIAD_HALF, -TRIAD_HALF, 0.1, 20);
	scene.add(new AmbientLight(0xffffff, 0.9));
	const light = new DirectionalLight(0xffffff, 2.4);
	light.position.set(-0.5, 0.8, 1);
	camera.add(light);
	scene.add(camera);
	const back = new Vector3();
	const ray = new Raycaster();
	const spot = new Vector2();
	return {
		scene,
		camera,
		look(q) {
			camera.quaternion.copy(q);
			camera.position.copy(back.set(0, 0, 8).applyQuaternion(q));
			camera.updateMatrixWorld();
		},
		faceAt(x, y) {
			ray.setFromCamera(spot.set(x, y), camera);
			const hit = ray.intersectObject(cube, false)[0];
			const side = hit?.face?.materialIndex;
			return side === undefined ? null : (CUBE_FACES[side] ?? null);
		},
		dispose() {
			geometry.dispose();
			(mesh.material as MeshLambertMaterial).dispose();
			cube.geometry.dispose();
			for (const side of sides) side.dispose();
		}
	};
}

/**
 * Where each arrow's label sits: a little past its free end, which is the head of an arrow that
 * points out and the tail of one that points in and ends on its node.
 */
export function labelPoints(arrows: { tails: ArrayLike<number>; directions: ArrayLike<number>; inward: ArrayLike<number>; length: number }, past = 0.18): Float32Array {
	const out = new Float32Array(arrows.tails.length);
	for (let k = 0; k < arrows.tails.length / 3; k++) {
		const reach = arrows.inward[k] ? -past * arrows.length : (1 + past) * arrows.length;
		for (let c = 0; c < 3; c++) out[3 * k + c] = arrows.tails[3 * k + c] + reach * arrows.directions[3 * k + c];
	}
	return out;
}
