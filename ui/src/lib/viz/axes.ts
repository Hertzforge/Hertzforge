/**
 * The labeled axes about a model, as Visual Dynamics' Plot axes draw them: the box at the
 * model's bounds carries tick marks and values along one edge per axis, the edge on the outside
 * of the box as seen from the camera, with the axis titled by its unit. The pane projects the
 * box's corners to the screen each frame; this places the labels on the picture.
 */

/** A point on the picture, in pixels from the top left. */
export interface Point {
	x: number;
	y: number;
}

/** The tick values between `lo` and `hi`, at a step of 1, 2 or 5 times a power of ten that gives about `want` of them, and the decimals they are written with. */
export function niceTicks(lo: number, hi: number, want = 4): { values: number[]; decimals: number } {
	if (!(hi > lo) || !Number.isFinite(lo) || !Number.isFinite(hi)) return { values: [lo], decimals: 0 };
	const raw = (hi - lo) / want;
	const power = 10 ** Math.floor(Math.log10(raw));
	const unit = raw / power;
	const step = (unit < 1.5 ? 1 : unit < 3.5 ? 2 : unit < 7.5 ? 5 : 10) * power;
	const decimals = Math.max(0, -Math.floor(Math.log10(step)));
	const values: number[] = [];
	for (let v = Math.ceil(lo / step - 1e-9) * step; v <= hi + step * 1e-9; v += step) {
		values.push(Number(v.toFixed(decimals)));
	}
	return { values, decimals };
}

/** An axis's labeled edge on the picture: the box corners it runs between, by index, and the unit direction pointing away from the box, where its labels go. */
export interface LabeledEdge {
	from: number;
	to: number;
	out: Point;
}

/**
 * For each axis, the edge of the box to label: of the four parallel to it, the one whose middle
 * lies lowest on the picture and, among those, outermost, so the labels sit below and beside the
 * model rather than across it. `corners` are the eight projected corners, indexed by bits (1 for
 * x at the high end, 2 for y, 4 for z); `centre` is the projected centre of the box. `out` points
 * from the box's middle through the edge's, where the labels go.
 */
export function labeledEdges(corners: Point[], centre: Point): LabeledEdge[] {
	return [1, 2, 4].map((bit) => {
		let best: LabeledEdge | null = null;
		let score = -Infinity;
		for (let from = 0; from < 8; from++) {
			if (from & bit) continue;
			const to = from | bit;
			const mx = (corners[from].x + corners[to].x) / 2 - centre.x;
			const my = (corners[from].y + corners[to].y) / 2 - centre.y;
			const s = my + Math.abs(mx);
			if (s > score) {
				score = s;
				const d = Math.hypot(mx, my);
				best = { from, to, out: d > 0 ? { x: mx / d, y: my / d } : { x: 0, y: 1 } };
			}
		}
		return best!;
	});
}

/** Where a value `v` in `lo`..`hi` falls on the edge from `a` to `b`. */
export function along(a: Point, b: Point, lo: number, hi: number, v: number): Point {
	const t = hi > lo ? (v - lo) / (hi - lo) : 0.5;
	return { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t };
}
