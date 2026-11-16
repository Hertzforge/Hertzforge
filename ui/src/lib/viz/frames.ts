/**
 * Averaging frames on a plot: each frame's window drawn in a slot on a rail along the top, and the
 * frame's shading following the window. Frames that overlap sit on rows of their own, so their
 * windows do not run through each other; the rail's proportions are the analysis core's.
 */

/** The rail, in fractions of the plot's height: where it starts, a slot's height, the room all rows share, and the window's share of its slot. */
export const RAIL = { top: 0.03, slot: 0.09, band: 0.34, glyph: 0.78 } as const;

/** The most a frame's shading darkens, where its window is 1. */
export const BAND_PEAK = 0.25;

/** One slot's height as a fraction of the plot's, for `rows` rows. */
export function slotHeight(rows: number): number {
	return Math.min(RAIL.slot, RAIL.band / Math.max(1, rows));
}

/** Where a frame's window sits on the rail: the top and the height of its glyph, as fractions of the plot's height. */
export function glyphBox(level: number, rows: number): { top: number; height: number } {
	const slot = slotHeight(rows);
	return { top: RAIL.top + level * slot + slot * (1 - RAIL.glyph), height: slot * RAIL.glyph };
}

/** A frame's shading from left to right: the frame edge's color at the window's weight. */
export function windowGradient(window: number[]): string {
	const last = Math.max(1, window.length - 1);
	const stops = window.map((w, k) => `color-mix(in srgb, var(--region-frame-edge) ${(100 * BAND_PEAK * Math.min(1, Math.max(0, w))).toFixed(1)}%, transparent) ${((100 * k) / last).toFixed(1)}%`);
	return `linear-gradient(to right, ${stops.join(', ')})`;
}

/** The window as a closed path in a 100 by 100 box, its baseline along the bottom. */
export function windowPath(window: number[]): string {
	const last = Math.max(1, window.length - 1);
	const points = window.map((w, k) => `${((100 * k) / last).toFixed(2)},${(100 - 100 * Math.min(1, Math.max(0, w))).toFixed(2)}`);
	return `M0,100 L${points.join(' L')} L100,100 Z`;
}

/** The window's glyph as an SVG element's markup, placed in its slot inside the frame's region. */
export function windowGlyph(window: number[], level: number, rows: number): string {
	const box = glyphBox(level, rows);
	return `<svg class="glyph" viewBox="0 0 100 100" preserveAspectRatio="none" style="top:${(100 * box.top).toFixed(2)}%;height:${(100 * box.height).toFixed(2)}%"><path d="${windowPath(window)}"/></svg>`;
}

/**
 * A frame's window on the stage's back wall, between stage x `a` and `b`: the triangles under the
 * window and the line along it, in its slot under the top of a wall `depth` back and `height` high.
 */
export function wallGlyph(window: number[], level: number, rows: number, a: number, b: number, depth: number, height: number): { faces: number[]; edges: number[] } {
	const faces: number[] = [];
	const edges: number[] = [];
	if (window.length < 2 || !(b > a)) return { faces, edges };
	const box = glyphBox(level, rows);
	const base = height * (1 - box.top - box.height);
	const tall = height * box.height;
	const last = window.length - 1;
	const at = (k: number): [number, number] => [a + ((b - a) * k) / last, base + tall * Math.min(1, Math.max(0, window[k]))];
	for (let k = 0; k < last; k++) {
		const [x0, z0] = at(k);
		const [x1, z1] = at(k + 1);
		faces.push(x0, depth, base, x1, depth, base, x1, depth, z1, x0, depth, base, x1, depth, z1, x0, depth, z0);
		edges.push(x0, depth, z0, x1, depth, z1);
	}
	edges.push(a, depth, base, b, depth, base);
	return { faces, edges };
}
