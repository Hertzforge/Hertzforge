/**
 * A shape view's lines, told apart by where they lie. A line along a side of a face is an
 * element's edge, drawn dark on the surface; any other line (a beam, a DOF arrow, a sensor
 * leg) keeps the line color, which reads against the background. The service sends them
 * together in `lines`; the triangles say which is which.
 */
export function splitOnFaces(lines: Uint32Array, triangles?: Uint32Array): { onFaces: Uint32Array; apart: Uint32Array } {
	if (!triangles || !triangles.length) return { onFaces: new Uint32Array(0), apart: lines };
	const sides = new Set<number>();
	// a side as one number, the smaller node first: exact while a model has under 2^26 nodes
	const key = (a: number, b: number) => (a < b ? a * 67108864 + b : b * 67108864 + a);
	for (let t = 0; t + 2 < triangles.length; t += 3) {
		const [a, b, c] = [triangles[t], triangles[t + 1], triangles[t + 2]];
		sides.add(key(a, b));
		sides.add(key(b, c));
		sides.add(key(c, a));
	}
	const on: number[] = [];
	const off: number[] = [];
	for (let l = 0; l + 1 < lines.length; l += 2) {
		(sides.has(key(lines[l], lines[l + 1])) ? on : off).push(lines[l], lines[l + 1]);
	}
	return { onFaces: Uint32Array.from(on), apart: Uint32Array.from(off) };
}
