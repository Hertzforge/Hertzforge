/**
 * How the mouse moves a 2D plot, after pyqtgraph's ViewBox, which the earlier toolset's plots
 * used (Brandon, 2026-11-14): a left (or middle) drag pans, a right drag stretches x by its
 * motion across and y by its motion up and down about where it was pressed, the wheel zooms both
 * axes about the pointer, and over an axis the drag or the wheel moves that axis alone. A right
 * click with no drag opens a menu with View All; the A at the corner does the same.
 */

/** The axes a press or a wheel at a point moves. */
export interface Axes {
	x: boolean;
	y: boolean;
}

/** A box on screen, as `getBoundingClientRect` gives it. */
export interface Box {
	left: number;
	top: number;
	right: number;
	bottom: number;
}

/**
 * The axes a point moves: both inside the plotting area, y alone over the y axis to its left, x
 * alone over the x axis below it, none elsewhere (the title, the legend beside it).
 */
export function axesAt(px: number, py: number, area: Box): Axes | null {
	const across = px >= area.left && px <= area.right;
	const down = py >= area.top && py <= area.bottom;
	if (across && down) return { x: true, y: true };
	if (down && px < area.left) return { x: false, y: true };
	if (across && py > area.bottom) return { x: true, y: false };
	return null;
}

/** Pixels a press may move and still be a click. */
export const CLICK_PX = 3;

/**
 * The factor the wheel zooms by, below 1 zooming in. pyqtgraph zooms by 1.02 to the power of
 * the wheel's turn in eighths of a degree over -8, a notch being 120 of them; a browser reports a
 * notch as about 100 pixels, so a notch zooms in to about 0.74 of the range, as pyqtgraph's does.
 */
export function wheelScale(deltaY: number, deltaMode = 0): number {
	const pixels = deltaMode === 1 ? deltaY * 33 : deltaMode === 2 ? deltaY * 800 : deltaY;
	return Math.min(4, Math.max(0.25, 1.02 ** (pixels * 0.15)));
}

/**
 * The factors a right drag of (dx, dy) pixels stretches x and y by, as pyqtgraph's: 1.02 per
 * pixel, a drag right or up zooming in.
 */
export function dragScale(dx: number, dy: number): [number, number] {
	return [1.02 ** -dx, 1.02 ** dy];
}

/**
 * The fractions of each range a left drag of (dx, dy) pixels pans by, on an area `width` by
 * `height`: the data follows the pointer, so a drag right shows lower x and a drag down higher y.
 */
export function panFractions(dx: number, dy: number, width: number, height: number): [number, number] {
	return [-dx / Math.max(1, width), dy / Math.max(1, height)];
}
