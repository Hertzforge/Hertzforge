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

/**
 * Decades a log level axis spans at most, below its highest value. A simulated plant's FRF falls
 * to 1e-14 at 0 Hz, and spanning that would press every resonance flat; anything lower than the
 * floor is drawn at the floor.
 */
export const LOG_DECADES = 8;

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
 * axis is linear), else the drawable values' own range. On a log axis the range is in real
 * values, never below the smallest positive level, and at most `LOG_DECADES` deep.
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
	if (log) lo = Math.max(lo, hi / 10 ** LOG_DECADES);
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

/**
 * The point under the cursor from a GPU pick: `pixels` is a `size` x `size` RGBA8 read back
 * around the cursor, each covered pixel holding its point's index plus one in little-endian
 * bytes and an empty one zero. Returns the point nearest the center, or null when none is.
 */
export function pickedPoint(pixels: Uint8Array, size: number): number | null {
	const c = (size - 1) / 2;
	let best: number | null = null;
	let bestDistance = Infinity;
	for (let j = 0; j < size; j++) {
		for (let i = 0; i < size; i++) {
			const k = 4 * (j * size + i);
			const id = pixels[k] + pixels[k + 1] * 256 + pixels[k + 2] * 65536 + pixels[k + 3] * 16777216;
			if (!id) continue;
			const distance = (i - c) ** 2 + (j - c) ** 2;
			if (distance < bestDistance) {
				bestDistance = distance;
				best = id - 1;
			}
		}
	}
	return best;
}

/**
 * Triangle indices for a surface: record r and r + 1 are joined between points i and i + 1 when
 * the two records have as many points and all four corners are drawable. A scalogram's rows are
 * on one clock, so every row pair joins.
 */
export function surfaceIndex(
	offsets: ArrayLike<number>,
	x: ArrayLike<number>,
	z: ArrayLike<number>,
	logX: boolean,
	logZ: boolean
): Uint32Array {
	const out: number[] = [];
	const ok = (i: number) => drawable(x[i], logX) && drawable(z[i], logZ);
	for (let r = 0; r + 2 < offsets.length; r++) {
		const a = offsets[r];
		const b = offsets[r + 1];
		const n = b - a;
		if (offsets[r + 2] - b !== n) continue;
		for (let i = 0; i + 1 < n; i++) {
			const p = a + i;
			const q = b + i;
			if (ok(p) && ok(p + 1) && ok(q) && ok(q + 1)) out.push(p, p + 1, q + 1, p, q + 1, q);
		}
	}
	return Uint32Array.from(out);
}

/** The station each point stands at: its record's, from `depths`, or the record itself. */
export function stationOf(offsets: ArrayLike<number>, points: number, depths?: ArrayLike<number>): Float32Array {
	const out = recordOf(offsets, points);
	if (depths) for (let i = 0; i < points; i++) out[i] = depths[out[i]];
	return out;
}

/** Where a value lands on the stage: x across, z up (null past the axis), station back. */
export interface StageMap {
	x(v: number): number | null;
	z(v: number): number | null;
	depth(station: number): number;
}

/** A map onto `STAGE` for these ranges; a level past the range is held at the top or floor. */
export function stageMap(xRange: [number, number], logX: boolean, zRange: [number, number], logZ: boolean, stations: number): StageMap {
	return {
		x: (v) => (drawable(v, logX) ? fraction(v, xRange, logX) * STAGE.x : null),
		z: (v) => (drawable(v, logZ) ? Math.min(1, Math.max(0, fraction(v, zRange, logZ))) * STAGE.z : null),
		depth: (s) => (stations > 1 ? (s / (stations - 1)) * STAGE.y : 0)
	};
}

function quad(out: number[], a: number[], b: number[], c: number[], d: number[]) {
	out.push(...a, ...b, ...c, ...a, ...c, ...d);
}

/**
 * A limit band's triangles at its station, between `inner` and `outer`, or out to the top (an
 * upper band) or floor (a lower one) when there is no `outer`. A segment with a gap at either end
 * is left out.
 */
export function zoneTriangles(
	zone: { station: number; side: 'upper' | 'lower'; x: ArrayLike<number>; inner: ArrayLike<number>; outer?: ArrayLike<number> },
	map: StageMap,
	out: number[] = []
): number[] {
	const y = map.depth(zone.station);
	const edge = zone.side === 'upper' ? STAGE.z : 0;
	const at = (i: number): [number, number, number] | null => {
		const x = map.x(zone.x[i]);
		const a = map.z(zone.inner[i]);
		const b = zone.outer ? map.z(zone.outer[i]) : edge;
		return x === null || a === null || b === null ? null : [x, a, b];
	};
	for (let i = 0; i + 1 < zone.x.length; i++) {
		const p = at(i);
		const q = at(i + 1);
		if (!p || !q) continue;
		quad(out, [p[0], y, p[1]], [q[0], y, q[1]], [q[0], y, q[2]], [p[0], y, p[2]]);
	}
	return out;
}

/** Each line past an abort limit as a rectangle across its bin, from the limit to the top or floor. */
export function exceedTriangles(
	hit: { station: number; side: 'upper' | 'lower'; x0: ArrayLike<number>; x1: ArrayLike<number>; z: ArrayLike<number> },
	map: StageMap,
	out: number[] = []
): number[] {
	const y = map.depth(hit.station);
	const edge = hit.side === 'upper' ? STAGE.z : 0;
	for (let i = 0; i < hit.x0.length; i++) {
		const a = map.x(hit.x0[i]);
		const b = map.x(hit.x1[i]);
		const z = map.z(hit.z[i]);
		if (a === null || b === null || z === null) continue;
		const lo = Math.max(0, Math.min(STAGE.x, a));
		const hi = Math.max(0, Math.min(STAGE.x, b));
		if (hi > lo) quad(out, [lo, y, z], [hi, y, z], [hi, y, edge], [lo, y, edge]);
	}
	return out;
}

/** A wall from floor to top through one abscissa a station, as a scalogram's cone is drawn. */
export function wallTriangles(wall: ArrayLike<number>, map: StageMap, out: number[] = []): number[] {
	for (let k = 0; k + 1 < wall.length; k++) {
		const a = map.x(wall[k]);
		const b = map.x(wall[k + 1]);
		if (a === null || b === null) continue;
		const ya = map.depth(k);
		const yb = map.depth(k + 1);
		quad(out, [a, ya, 0], [b, yb, 0], [b, yb, STAGE.z], [a, ya, STAGE.z]);
	}
	return out;
}
