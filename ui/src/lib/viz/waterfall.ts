/**
 * The waterfall's arithmetic, kept out of the component so it can be tested without a GPU: which
 * points join into segments, the level range the colors span, the ticks, and which records are
 * named. The vertex shader repeats the axis mapping (`stageX`, `stageZ`) on the GPU, so changing
 * an axis between linear and log moves no data.
 */

import { logTicks } from './logTicks';

/** At most this many record labels; past it every nth record is named (Visual Dynamics' limit). */
export const LABEL_LIMIT = 40;

/** Stage size: abscissa across, records receding, level up. Wide and shallow, as in VD. */
export const STAGE = { x: 1.6, y: 0.9, z: 0.5 } as const;

/** Whether a value can be drawn on an axis: finite, and positive when the axis is log. */
export function drawable(v: number, log: boolean): boolean {
	return Number.isFinite(v) && (!log || v > 0);
}

/**
 * Index pairs for one LineSegments draw: consecutive points of a record join when both are
 * drawable, so a gap (NaN, or a level at or below zero on a log axis) breaks the line, and no
 * segment ever crosses from one record to the next.
 */
export function segmentIndex(
	offsets: ArrayLike<number>,
	x: ArrayLike<number>,
	z: ArrayLike<number>,
	logX: boolean,
	logZ: boolean
): Uint32Array {
	const records = offsets.length - 1;
	let count = 0;
	for (let r = 0; r < records; r++) {
		for (let i = offsets[r]; i + 1 < offsets[r + 1]; i++) {
			if (drawable(x[i], logX) && drawable(x[i + 1], logX) && drawable(z[i], logZ) && drawable(z[i + 1], logZ)) count++;
		}
	}
	const out = new Uint32Array(2 * count);
	let k = 0;
	for (let r = 0; r < records; r++) {
		for (let i = offsets[r]; i + 1 < offsets[r + 1]; i++) {
			if (drawable(x[i], logX) && drawable(x[i + 1], logX) && drawable(z[i], logZ) && drawable(z[i + 1], logZ)) {
				out[k++] = i;
				out[k++] = i + 1;
			}
		}
	}
	return out;
}

/** The record each point belongs to, for the depth attribute. */
export function recordOf(offsets: ArrayLike<number>, points: number): Float32Array {
	const out = new Float32Array(points);
	for (let r = 0; r + 1 < offsets.length; r++) out.fill(r, offsets[r], offsets[r + 1]);
	return out;
}

/**
 * The level range the stage and the colors span: the pinned range when the data pins one (and the
 * axis is linear), else the drawable values' own range. On a log axis the range is in decades'
 * real values, never below the smallest positive level.
 */
export function levelRange(z: ArrayLike<number>, log: boolean, pinned?: [number, number]): [number, number] {
	if (pinned && !log) return pinned;
	let lo = Infinity;
	let hi = -Infinity;
	for (let i = 0; i < z.length; i++) {
		const v = z[i];
		if (!drawable(v, log)) continue;
		if (v < lo) lo = v;
		if (v > hi) hi = v;
	}
	if (!(hi >= lo)) return log ? [1, 10] : [0, 1];
	if (hi === lo) return log ? [lo / 10 ** 0.5, hi * 10 ** 0.5] : [lo - 0.5 || -0.5, hi + 0.5];
	return [lo, hi];
}

/** A value's place on an axis span, 0 at `range[0]` and 1 at `range[1]`, in log10 when `log`. */
export function fraction(v: number, range: [number, number], log: boolean): number {
	if (log) {
		const a = Math.log10(range[0]);
		const b = Math.log10(range[1]);
		return (Math.log10(v) - a) / (b - a);
	}
	return (v - range[0]) / (range[1] - range[0]);
}

/** Round linear ticks: about `target` of them, on 1, 2, 2.5 or 5 times a power of ten. */
export function linearTicks(lo: number, hi: number, target = 5): number[] {
	const span = hi - lo;
	if (!(span > 0) || !Number.isFinite(span)) return [lo];
	const raw = span / target;
	const mag = 10 ** Math.floor(Math.log10(raw));
	const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => raw <= s) ?? 10 * mag;
	const out: number[] = [];
	for (let v = Math.ceil(lo / step - 1e-9) * step; v <= hi + step * 1e-9 && out.length < 50; v += step) {
		out.push(Number(v.toPrecision(12)));
	}
	return out;
}

/** Ticks for an axis span, decades on a log axis. */
export function axisTicks(range: [number, number], log: boolean): number[] {
	if (!log) return linearTicks(range[0], range[1]);
	const ticks = logTicks(range[0], range[1], [1]);
	return ticks.length >= 2 ? ticks : logTicks(range[0], range[1]);
}

/** Which records get a label: every one up to `limit`, then every nth, always the last. */
export function namedRecords(count: number, limit = LABEL_LIMIT): number[] {
	if (count <= limit) return Array.from({ length: count }, (_, i) => i);
	const stride = Math.ceil(count / limit);
	const out: number[] = [];
	for (let i = 0; i < count; i += stride) out.push(i);
	if (out[out.length - 1] !== count - 1) out.push(count - 1);
	return out;
}
