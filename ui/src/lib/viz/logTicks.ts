/**
 * Closed-form log-axis ticks. uPlot's own log splitter loops without bound on a non-positive or
 * non-finite scale minimum, so every log axis in the kit supplies this instead. Bounded by
 * construction and safe on degenerate input.
 */

const MAX_TICKS = 120;
const FLOOR = 1e-15;

/**
 * Decade ticks plus 2 and 5 within a decade when the span is narrow enough to show them. With
 * `stride` above 1, only every stride-th decade is ticked (a short axis over many decades).
 */
export function logTicks(min: number, max: number, mantissas?: number[], stride = 1): number[] {
	let lo = Number.isFinite(min) && min > 0 ? min : FLOOR;
	let hi = Number.isFinite(max) && max > 0 ? max : lo * 10;
	if (hi <= lo) hi = lo * 10;
	const e0 = Math.floor(Math.log10(lo));
	const e1 = Math.min(Math.ceil(Math.log10(hi)), e0 + MAX_TICKS);
	const span = e1 - e0;
	const ms = mantissas ?? (span <= 2 ? [1, 2, 5] : span <= 5 ? [1, 3] : [1]);
	const out: number[] = [];
	const every = Math.max(1, Math.floor(stride));
	for (let e = e0; e <= e1 && out.length < MAX_TICKS; e++) {
		if (((e % every) + every) % every !== 0) continue;
		for (const m of ms) {
			const v = m * 10 ** e;
			// Round away float noise so labels read 0.003, not 0.0030000000000000005.
			const r = Number(v.toPrecision(12));
			if (r >= lo * (1 - 1e-9) && r <= hi * (1 + 1e-9)) out.push(r);
		}
	}
	return out;
}

/** Mantissas that fit `pxPerDecade` of axis without crowding the labels. */
export function mantissasFor(pxPerDecade: number): number[] {
	if (pxPerDecade >= 190) return [1, 2, 5];
	if (pxPerDecade >= 95) return [1, 3];
	return [1];
}

/** Smallest room a tick label needs on an axis, in CSS pixels. */
const MIN_TICK_PX = 24;

/** Tick every n-th decade so labels never sit closer than `MIN_TICK_PX`. */
export function decadeStride(pxPerDecade: number): number {
	if (!(pxPerDecade > 0)) return 1;
	return pxPerDecade >= MIN_TICK_PX ? 1 : Math.ceil(MIN_TICK_PX / pxPerDecade);
}

/** A log range snapped outward to decades (or half decades when the data spans less than one). */
export function logRange(min: number, max: number): [number, number] {
	let lo = Number.isFinite(min) && min > 0 ? min : FLOOR;
	let hi = Number.isFinite(max) && max > 0 ? max : lo * 10;
	if (hi <= lo) hi = lo * 10;
	const step = Math.log10(hi / lo) < 1.5 ? 0.5 : 1;
	const a = Math.floor(Math.log10(lo) / step) * step;
	let b = Math.ceil(Math.log10(hi) / step) * step;
	if (b <= a) b = a + step;
	return [10 ** a, 10 ** b];
}
