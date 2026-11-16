/**
 * Averaging frames on a plot: each frame's window drawn in a slot on a rail along the top, and the
 * frame's shading following the window. Frames that overlap sit on rows of their own, so their
 * windows do not run through each other; the rail's proportions are the analysis core's.
 */

import type { Region } from '$lib/protocol/types';

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

/**
 * A frame's shading on the stage, between stage x `a` and `b`: strips across the floor and up the
 * back wall, with the opacity at each corner the window's weight there (`alphas`, one per vertex),
 * so the stage is shaded as the flat plot is and overlapping frames add. A frame that runs past
 * the stage, which ends at x `width`, is cut there, with the weight the window has at the cut.
 */
export function windowBands(window: number[], a: number, b: number, depth: number, height: number, width = Infinity): { faces: number[]; alphas: number[] } {
	const faces: number[] = [];
	const alphas: number[] = [];
	if (window.length < 2 || !(b > a)) return { faces, alphas };
	const last = window.length - 1;
	const weight = (k: number) => BAND_PEAK * Math.min(1, Math.max(0, window[k]));
	for (let k = 0; k < last; k++) {
		const from = a + ((b - a) * k) / last;
		const to = a + ((b - a) * (k + 1)) / last;
		const x0 = Math.max(0, from);
		const x1 = Math.min(width, to);
		if (!(x1 > x0)) continue;
		const at = (x: number) => weight(k) + ((weight(k + 1) - weight(k)) * (x - from)) / (to - from);
		const w0 = at(x0);
		const w1 = at(x1);
		// the floor, then the wall
		faces.push(x0, 0, 0, x1, 0, 0, x1, depth, 0, x0, 0, 0, x1, depth, 0, x0, depth, 0);
		alphas.push(w0, w1, w1, w0, w1, w0);
		faces.push(x0, depth, 0, x1, depth, 0, x1, depth, height, x0, depth, 0, x1, depth, height, x0, depth, height);
		alphas.push(w0, w1, w1, w0, w1, w0);
	}
	return { faces, alphas };
}

/** Which part of a span is being dragged: its low edge, the whole of it, or its high edge. */
export type DragPart = 'low' | 'move' | 'high';

/** How many frames a span from `x0` to `x1` holds. */
function framesIn(snap: NonNullable<Region['snap']>, x0: number, x1: number): number {
	return Math.max(1, 1 + Math.floor((x1 - x0 - snap.frame) / snap.hop + 1e-6));
}

/**
 * The whole frames a span being dragged would leave: where they start and end, and how many. The
 * middle dragged keeps the count and stays on the record; an edge dragged adds or removes whole
 * frames at that end, at least one, the other end staying where it was.
 */
export function draggedSpan(snap: NonNullable<Region['snap']>, part: DragPart, was: { x0: number; x1: number }, x0: number, x1: number): { x0: number; x1: number; count: number } {
	const length = (count: number) => snap.frame + (count - 1) * snap.hop;
	if (part === 'move') {
		const count = framesIn(snap, was.x0, was.x1);
		const start = Math.max(snap.low, Math.min(x0, snap.high - length(count)));
		return { x0: start, x1: start + length(count), count };
	}
	if (part === 'high') {
		const count = Math.min(framesIn(snap, was.x0, x1), framesIn(snap, was.x0, snap.high));
		return { x0: was.x0, x1: was.x0 + length(count), count };
	}
	const count = Math.min(framesIn(snap, x0, was.x1), framesIn(snap, snap.low, was.x1));
	return { x0: was.x1 - length(count), x1: was.x1, count };
}

/** Past this many frames a preview is one shaded span: a frame a pixel wide says nothing. */
export const MOST_FRAMES_DRAWN = 1000;

/**
 * The frames a drag would leave, as regions to draw in place of the ones drawn before it: each
 * with the window the frames have and its row among the frames it overlaps.
 */
export function previewFrames(regions: Region[], snap: NonNullable<Region['snap']>, span: { x0: number; count: number }): Region[] {
	const like = regions.find((r) => r.kind === 'frame' && r.window?.length);
	if (span.count > MOST_FRAMES_DRAWN || !like) return [{ x0: span.x0, x1: span.x0 + snap.frame + (span.count - 1) * snap.hop, kind: 'frame' }];
	// frames a hop apart overlap the next few: each takes the row after the last
	const rows = Math.min(span.count, Math.max(1, Math.ceil(snap.frame / snap.hop - 1e-9)));
	return Array.from({ length: span.count }, (_, k) => {
		const x0 = span.x0 + k * snap.hop;
		return { x0, x1: x0 + snap.frame, kind: 'frame' as const, window: like.window, level: k % rows, rows };
	});
}

/** How many frames, in words. */
export function framesText(count: number): string {
	return `${count} frame${count === 1 ? '' : 's'}`;
}

/** The share of a plot's height the rail takes, for the rows its regions have; 0 with no windows. */
export function railShare(regions: Region[]): number {
	const rows = Math.max(0, ...regions.filter((r) => r.window?.length).map((r) => r.rows ?? 1));
	return rows ? RAIL.top + rows * slotHeight(rows) + 0.02 : 0;
}
