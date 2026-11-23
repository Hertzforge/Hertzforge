/**
 * An element shape set drawn on its model (`element_values`): each element's outside faces as
 * their own triangles, so every element keeps its own flat color, riding the mode's deflection
 * with the surface. Strain and stress jump from one element to the next, which is why the faces
 * are colored per element and not per node. The values arrive from the service, one per element
 * and mode; the shell only places them on the colormap, as it places a node's travel.
 */
import { colormap } from './colormap';

/** Each triangle corner its own vertex: the nodes' f32 [N, 3] rows at the triangles' corners. */
export function cornerRows(rows: Float32Array, triangles: Uint32Array, out = new Float32Array(3 * triangles.length)): Float32Array {
	for (let v = 0; v < triangles.length; v++) out.set(rows.subarray(3 * triangles[v], 3 * triangles[v] + 3), 3 * v);
	return out;
}

/** The largest magnitude among one mode's values (row `row` of `count` per row), or 0. */
export function rowPeak(values: Float32Array, row: number, count: number): number {
	let peak = 0;
	for (let e = row * count; e < (row + 1) * count && e < values.length; e++) {
		const a = Math.abs(values[e]);
		if (Number.isFinite(a) && a > peak) peak = a;
	}
	return peak;
}

function srgbToLinear(c: number) {
	return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

/**
 * Color every corner by its element's value in mode `row`, `perRow` elements a mode: from minus the
 * mode's largest magnitude at the map's low end to plus it at the high end, zero in the middle,
 * linear RGB for the vertex colors. A mode whose elements all read zero is drawn at the middle.
 * Returns the peak.
 */
export function paintElements(
	values: Float32Array,
	row: number,
	perRow: number,
	owner: Uint32Array,
	out: Float32Array,
	low?: number
): number {
	const peak = rowPeak(values, row, perRow);
	const rgb: [number, number, number] = [0, 0, 0];
	for (let t = 0; t < owner.length; t++) {
		const value = values[row * perRow + owner[t]] ?? 0;
		colormap(peak > 0 ? 0.5 + (0.5 * value) / peak : 0.5, rgb, low);
		for (let corner = 0; corner < 3; corner++)
			for (let c = 0; c < 3; c++) out[9 * t + 3 * corner + c] = srgbToLinear(rgb[c]);
	}
	return peak;
}
