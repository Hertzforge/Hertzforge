/**
 * What a click in the 3-D view lands on, kept out of the component so it can be tested without a
 * GPU: the node nearest the cursor on screen, where a placed point goes when no surface is under
 * the cursor, and the params a click on a surface changes.
 */

/** How near the cursor a node must be on screen to be picked, in CSS pixels (Visual Dynamics' 12). */
export const PICK_REACH = 12;

/**
 * The row of the node nearest (x, y) among the first `count`, given each node's place on screen
 * (`screen`, x and y a node) and whether it is in front of the camera (`visible`), within `reach`.
 */
export function nearestNode(screen: ArrayLike<number>, visible: ArrayLike<boolean>, count: number, x: number, y: number, reach = PICK_REACH): number | null {
	let best: number | null = null;
	let bestDistance = reach * reach;
	for (let n = 0; n < count; n++) {
		if (!visible[n]) continue;
		const d = (screen[2 * n] - x) ** 2 + (screen[2 * n + 1] - y) ** 2;
		if (d <= bestDistance) {
			bestDistance = d;
			best = n;
		}
	}
	return best;
}

type Vec = [number, number, number];

const dot = (a: Vec, b: Vec) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const sub = (a: Vec, b: Vec): Vec => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const cross = (a: Vec, b: Vec): Vec => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];

/**
 * The plane a flat model lies in, as a point on it and its unit normal, or null when the model is
 * not flat: one whose nodes are all within a millionth of its size of the plane through three of
 * them spread widest.
 */
export function modelPlane(nodes: ArrayLike<number>, count: number): { point: Vec; normal: Vec } | null {
	if (count < 3) return null;
	const at = (n: number): Vec => [nodes[3 * n], nodes[3 * n + 1], nodes[3 * n + 2]];
	const a = at(0);
	let far = 0;
	let b = a;
	for (let n = 1; n < count; n++) {
		const d = dot(sub(at(n), a), sub(at(n), a));
		if (d > far) {
			far = d;
			b = at(n);
		}
	}
	if (!(far > 0)) return null;
	let normal: Vec = [0, 0, 0];
	let widest = 0;
	for (let n = 1; n < count; n++) {
		const c = cross(sub(b, a), sub(at(n), a));
		const area = dot(c, c);
		if (area > widest) {
			widest = area;
			normal = c;
		}
	}
	if (!(widest > 0)) return null;
	const length = Math.sqrt(widest);
	normal = [normal[0] / length, normal[1] / length, normal[2] / length];
	const size = Math.sqrt(far);
	for (let n = 0; n < count; n++) if (Math.abs(dot(sub(at(n), a), normal)) > 1e-6 * size) return null;
	return { point: a, normal };
}

/**
 * Where a point clicked along the ray (`origin` + t `direction`) goes when it hits no surface,
 * as Visual Dynamics places one: on the model's plane when it is flat and the ray crosses it,
 * otherwise at the depth of `anchor`, the node nearest the click, so it lands near what was
 * clicked.
 */
export function placedPoint(origin: Vec, direction: Vec, anchor: Vec, plane: { point: Vec; normal: Vec } | null): Vec {
	const along = (t: number): Vec => [origin[0] + t * direction[0], origin[1] + t * direction[1], origin[2] + t * direction[2]];
	if (plane) {
		const facing = dot(direction, plane.normal);
		if (Math.abs(facing) > 1e-9) return along(dot(sub(plane.point, origin), plane.normal) / facing);
	}
	return along(dot(sub(anchor, origin), direction) / dot(direction, direction));
}

/** A comma list of ids, as the service reads one: '4, 7, 9'. */
export function idList(ids: number[]): string {
	return ids.join(', ');
}

function idsIn(text: unknown): number[] {
	return String(text ?? '')
		.split(',')
		.map((t) => t.trim())
		.filter(Boolean)
		.map(Number)
		.filter((n) => Number.isInteger(n));
}

/**
 * The params a click on surface `id` changes: one in use (`kept`) is named in `out` and dropped
 * from `back`; any other is named in `back` and dropped from `out`.
 */
export function toggledSurface(params: Record<string, unknown>, kept: number[], out: string, back: string, id: number): Record<string, string> {
	const leaving = kept.includes(id);
	const add = leaving ? out : back;
	const drop = leaving ? back : out;
	const added = idsIn(params[add]).filter((n) => n !== id);
	return { [add]: idList([...added, id]), [drop]: idList(idsIn(params[drop]).filter((n) => n !== id)) };
}

/** Meters in a length unit the view can be drawn in. */
const METERS: Record<string, number> = { m: 1, cm: 0.01, mm: 0.001, in: 0.0254, ft: 0.3048 };

/**
 * The grid an arrow's drag lands on, in the view's `unit`: a tenth of an inch for the inch family,
 * a centimeter otherwise, so a dragged part lands on round numbers. None for an undefined geometry,
 * whose numbers have no unit to round in.
 */
export function gridStep(unit: string): number | null {
	const meters = METERS[unit];
	if (meters === undefined) return null;
	return (unit === 'in' || unit === 'ft' ? 0.00254 : 0.01) / meters;
}
