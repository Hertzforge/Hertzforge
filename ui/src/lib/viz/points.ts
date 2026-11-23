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

/** An amplitude of 1, as a fraction of the model's diagonal: the 3-D view's slider's default position, and its scale. */
export const AMP_UNIT = 0.075;

/**
 * The deflection an amplitude of 1 draws, in the model's units: `AMP_UNIT` of the diagonal across
 * the nodes, point layers and segment layers. It is the whole model's whatever the view is framed
 * on (`home.frame`), so framing closer does not change how far the motion is magnified; 1 when
 * nothing has an extent.
 */
export function amplitudeUnit(view: { nodes: Float32Array; points?: { xyz: Float32Array }[]; segments?: { xyz: Float32Array }[] }): number {
	const { lo, hi } = span([view.nodes, ...(view.points ?? []).map((layer) => layer.xyz), ...(view.segments ?? []).map((layer) => layer.xyz)]);
	// no nodes (only bodies drawn) gives an infinite diagonal, and Infinity × 0 a NaN that puts
	// every point nowhere
	const across = Math.hypot(hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]);
	return AMP_UNIT * (Number.isFinite(across) && across > 0 ? across : 1);
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
