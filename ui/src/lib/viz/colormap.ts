/**
 * Sequential colormap for magnitude: viridis, dark purple through blue and green to yellow, as
 * matplotlib draws it (Brandon, 2026-11-02, over the cividis the shell had).
 * Designed to read the same with red-green color vision deficiency, and monotonic in lightness,
 * so the order of values survives in grayscale too.
 */

// matplotlib's viridis at eleven even steps
const STOPS: [number, number, number][] = [
	[68, 1, 84],
	[72, 36, 117],
	[65, 68, 135],
	[53, 95, 141],
	[42, 120, 142],
	[33, 145, 140],
	[34, 168, 132],
	[68, 191, 112],
	[122, 209, 81],
	[189, 223, 38],
	[253, 231, 37]
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
