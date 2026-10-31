/**
 * Spectral lines drawn the way they are computed: each value is a density over its own bin,
 * flat from one edge to the next, so the area under the trace is the area a PSD integrates to.
 * The service sends the edges; this only places them on the canvas. A bin with no value (NaN,
 * or not positive on a log axis) breaks the trace rather than bridging it.
 */

import type uPlot from 'uplot';
import type { NumericArray } from '$lib/protocol/types';

/** One unbroken run of the outline as [x, y] data points, two per bin. */
export type StepRun = [number, number][];

/**
 * The outline's vertices: for each drawable bin, its left and right edge at its value. Adjacent
 * drawable bins share an edge, so drawing a run as a polyline gives the vertical riser between
 * them. On a log x axis an edge at or below zero is pulled in to its bin's right edge's tenth,
 * the same place a mirrored first edge of a DC-free spectrum would be.
 */
export function stepRuns(edges: NumericArray, y: NumericArray, logX: boolean, logY: boolean): StepRun[] {
	const n = Math.min(y.length, edges.length - 1);
	const runs: StepRun[] = [];
	let run: StepRun = [];
	const close = () => {
		if (run.length) runs.push(run);
		run = [];
	};
	for (let i = 0; i < n; i++) {
		const v = y[i];
		let left = edges[i];
		const right = edges[i + 1];
		const ok = Number.isFinite(v) && (!logY || v > 0) && Number.isFinite(left) && Number.isFinite(right) && right > left;
		if (!ok || (logX && right <= 0)) {
			close();
			continue;
		}
		if (logX && left <= 0) left = right / 10;
		run.push([left, v], [right, v]);
	}
	close();
	return runs;
}

/**
 * A uPlot path builder that strokes `stepRuns`, clipped to the plotting area. `current` is read
 * on every draw, so live data replaced through `setData` is drawn from its own edges.
 */
export function stepsPath(
	current: () => { edges?: NumericArray; y: NumericArray } | undefined,
	logX: boolean,
	logY: boolean
): uPlot.Series.PathBuilder {
	return (u) => {
		const s = current();
		const runs = s?.edges ? stepRuns(s.edges, s.y, logX, logY) : [];
		const stroke = new Path2D();
		for (const run of runs) {
			run.forEach(([xv, yv], k) => {
				const px = u.valToPos(xv, 'x', true);
				const py = u.valToPos(yv, 'y', true);
				if (k === 0) stroke.moveTo(px, py);
				else stroke.lineTo(px, py);
			});
		}
		const clip = new Path2D();
		clip.rect(u.bbox.left, u.bbox.top, u.bbox.width, u.bbox.height);
		return { stroke, fill: null, clip, flags: 0 };
	};
}

/** The value of the bin that contains x, or null outside the edges. */
export function binValueAt(edges: NumericArray, y: NumericArray, x: number): number | null {
	const n = Math.min(y.length, edges.length - 1);
	if (n <= 0 || !Number.isFinite(x) || x < edges[0] || x > edges[n]) return null;
	let lo = 0;
	let hi = n;
	while (hi - lo > 1) {
		const mid = (lo + hi) >> 1;
		if (edges[mid] <= x) lo = mid;
		else hi = mid;
	}
	const v = y[lo];
	return Number.isFinite(v) ? v : null;
}
