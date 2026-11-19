/** Free points in a `shape3d` view (`view.points`): what the pane frames and how it colors them. */

/** The lowest and highest coordinate along each axis over flat xyz arrays; infinite when none holds a point. */
export function span(arrays: Float32Array[]): { lo: number[]; hi: number[] } {
	const lo = [Infinity, Infinity, Infinity];
	const hi = [-Infinity, -Infinity, -Infinity];
	for (const n of arrays) {
		for (let i = 0; i + 2 < n.length; i += 3) {
			for (let k = 0; k < 3; k++) {
				if (n[i + k] < lo[k]) lo[k] = n[i + k];
				if (n[i + k] > hi[k]) hi[k] = n[i + k];
			}
		}
	}
	return { lo, hi };
}

/** sRGB bytes as the linear floats a vertex color attribute takes. */
export function linearColors(srgb: Uint8Array): Float32Array {
	const out = new Float32Array(srgb.length);
	for (let i = 0; i < srgb.length; i++) {
		const c = srgb[i] / 255;
		out[i] = c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
	}
	return out;
}
