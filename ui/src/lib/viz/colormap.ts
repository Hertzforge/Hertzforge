/**
 * Sequential colormap for magnitude: a cividis-like ramp, dark blue through gray to yellow.
 * Designed to read the same with red-green color vision deficiency, and monotonic in lightness,
 * so the order of values survives in grayscale too.
 */

const STOPS: [number, number, number][] = [
	[0, 34, 78],
	[24, 55, 111],
	[61, 77, 110],
	[90, 97, 111],
	[117, 117, 117],
	[143, 138, 120],
	[171, 160, 114],
	[201, 184, 98],
	[234, 211, 72],
	[254, 232, 56]
];

/** RGB in 0..1 for t in 0..1 (clamped). */
export function colormap(t: number, out: [number, number, number] = [0, 0, 0]): [number, number, number] {
	const c = Math.min(1, Math.max(0, Number.isFinite(t) ? t : 0)) * (STOPS.length - 1);
	const i = Math.min(STOPS.length - 2, Math.floor(c));
	const f = c - i;
	const a = STOPS[i];
	const b = STOPS[i + 1];
	out[0] = (a[0] + (b[0] - a[0]) * f) / 255;
	out[1] = (a[1] + (b[1] - a[1]) * f) / 255;
	out[2] = (a[2] + (b[2] - a[2]) * f) / 255;
	return out;
}

/** CSS linear-gradient stops for a legend bar. */
export function colormapGradient(direction = 'to right'): string {
	const stops = STOPS.map(([r, g, b], i) => `rgb(${r} ${g} ${b}) ${((i / (STOPS.length - 1)) * 100).toFixed(1)}%`);
	return `linear-gradient(${direction}, ${stops.join(', ')})`;
}
