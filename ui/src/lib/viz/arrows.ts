/**
 * Solid arrows: a round shaft and a cone head, shaded by the scene's lights. The DOF arrows over a
 * model and the orientation axes in the corner are both made of them, in the axes' colors (X red,
 * Y green, Z blue).
 */
import {
	AmbientLight,
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
	Scene,
	SphereGeometry,
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

/**
 * The orientation axes: three solid arrows from a ball at the origin, with their own light and a
 * camera that only turns. `look` points the camera the way the view's camera looks.
 */
export function triadScene(): { scene: Scene; camera: OrthographicCamera; look: (q: Quaternion) => void; dispose: () => void } {
	const scene = new Scene();
	const stout = { shaft: 0.07, head: 0.36, headRadius: 0.17 };
	const geometry = arrowGeometry(stout);
	const mesh = arrowMesh({ tails: [0, 0, 0, 0, 0, 0, 0, 0, 0], directions: [1, 0, 0, 0, 1, 0, 0, 0, 1], axes: [0, 1, 2], length: 1 });
	mesh.geometry.dispose();
	mesh.geometry = geometry;
	const ball = new Mesh(new SphereGeometry(0.12, 20, 14), new MeshLambertMaterial({ color: 0x9a9a9a }));
	scene.add(mesh, ball);
	const camera = new OrthographicCamera(-TRIAD_HALF, TRIAD_HALF, TRIAD_HALF, -TRIAD_HALF, 0.1, 20);
	scene.add(new AmbientLight(0xffffff, 0.9));
	const light = new DirectionalLight(0xffffff, 2.4);
	light.position.set(-0.5, 0.8, 1);
	camera.add(light);
	scene.add(camera);
	const back = new Vector3();
	return {
		scene,
		camera,
		look(q) {
			camera.quaternion.copy(q);
			camera.position.copy(back.set(0, 0, 8).applyQuaternion(q));
			camera.updateMatrixWorld();
		},
		dispose() {
			geometry.dispose();
			(mesh.material as MeshLambertMaterial).dispose();
			ball.geometry.dispose();
			ball.material.dispose();
		}
	};
}
