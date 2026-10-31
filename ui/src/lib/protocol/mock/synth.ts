/**
 * Deterministic synthetic data for mock mode. Everything here stands in for a service: it is
 * the only numerics in the shell, it never ships as a real result, and the same inputs always
 * give the same output so screenshots and visual tests are stable.
 */

/** Small, fast, seedable PRNG (mulberry32). */
export function rng(seed: number): () => number {
	let a = seed >>> 0;
	return () => {
		a = (a + 0x6d2b79f5) >>> 0;
		let t = a;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

export function hashSeed(...parts: (string | number | boolean)[]): number {
	let h = 2166136261;
	for (const ch of parts.join('|')) {
		h ^= ch.charCodeAt(0);
		h = Math.imul(h, 16777619);
	}
	return h >>> 0;
}

/** Standard normal sample from a uniform source (Box-Muller). */
export function gauss(next: () => number): number {
	const u = Math.max(next(), 1e-12);
	return Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * next());
}

// ---------------------------------------------------------------------------------------------
// Random vibration

export type Breakpoints = [number, number][];

export const QUAL_SPEC: Breakpoints = [
	[20, 0.01],
	[80, 0.04],
	[350, 0.04],
	[2000, 0.007]
];

export const WORKMANSHIP_SPEC: Breakpoints = [
	[20, 0.01],
	[80, 0.04],
	[500, 0.04],
	[2000, 0.01]
];

/** Log-log interpolation through the breakpoints (straight lines on a log-log plot). */
export function specAt(spec: Breakpoints, f: number): number {
	if (f <= spec[0][0]) return spec[0][1];
	for (let i = 1; i < spec.length; i++) {
		const [f1, p1] = spec[i];
		if (f <= f1) {
			const [f0, p0] = spec[i - 1];
			const t = Math.log(f / f0) / Math.log(f1 / f0);
			return Math.exp(Math.log(p0) + t * Math.log(p1 / p0));
		}
	}
	return spec[spec.length - 1][1];
}

/** Frequency lines: 5 Hz resolution, 20 to 2000 Hz (397 lines). */
export function frequencyLines(f0 = 20, f1 = 2000, df = 5): Float64Array {
	const n = Math.floor((f1 - f0) / df) + 1;
	const out = new Float64Array(n);
	for (let i = 0; i < n; i++) out[i] = f0 + i * df;
	return out;
}

/** Overall rms from a PSD on its lines (trapezoid). */
export function rms(f: ArrayLike<number>, psd: ArrayLike<number>): number {
	let area = 0;
	for (let i = 1; i < f.length; i++) area += 0.5 * (psd[i] + psd[i - 1]) * (f[i] - f[i - 1]);
	return Math.sqrt(area);
}

/** Exact rms of a breakpoint specification (each segment is a power law). */
export function specRms(spec: Breakpoints): number {
	let area = 0;
	for (let i = 1; i < spec.length; i++) {
		const [f0, p0] = spec[i - 1];
		const [f1, p1] = spec[i];
		const k = Math.log(p1 / p0) / Math.log(f1 / f0);
		area += Math.abs(k + 1) < 1e-9 ? p0 * f0 * Math.log(f1 / f0) : (p0 / (k + 1)) * (f1 * (f1 / f0) ** k - f0);
	}
	return Math.sqrt(area);
}

/** Breakpoints scaled by `db` (10 log10 on a PSD). */
export function scaleSpec(spec: Breakpoints, db: number): Breakpoints {
	const g = 10 ** (db / 10);
	return spec.map(([f, p]) => [f, p * g]);
}

export function breakpointArrays(spec: Breakpoints): { x: Float64Array; y: Float64Array } {
	return { x: Float64Array.from(spec.map((b) => b[0])), y: Float64Array.from(spec.map((b) => b[1])) };
}

/** Base-excitation transmissibility magnitude squared of one mode. */
export function sdofT2(f: number, fn: number, q: number): number {
	const r = f / fn;
	const z = 1 / (2 * q);
	const num = 1 + (2 * z * r) ** 2;
	const den = (1 - r * r) ** 2 + (2 * z * r) ** 2;
	return num / den;
}

export interface Resonance {
	fn: number;
	q: number;
}

export interface ChannelModel {
	value: string;
	label: string;
	/** Empty for a control channel; modes in series for a response channel. */
	modes: Resonance[];
}

/** One run's channel set. Response channels ride on control 1. */
export const RUN_CHANNELS: ChannelModel[] = [
	{ value: 'ctrl1', label: 'Control 1', modes: [] },
	{ value: 'ctrl2', label: 'Control 2', modes: [] },
	{ value: 'resp1', label: 'Response 1, Bracket', modes: [{ fn: 185, q: 14 }, { fn: 720, q: 22 }] },
	{ value: 'resp2', label: 'Response 2, Top Plate', modes: [{ fn: 310, q: 10 }, { fn: 1250, q: 18 }] }
];

/**
 * A measured PSD for `channel` of a run: the specification shaped by a slow equalization ripple,
 * one region the loop under-drives, estimation scatter, and (for responses) the channel's modes.
 */
export function channelPsd(
	f: Float64Array,
	spec: Breakpoints,
	channel: ChannelModel,
	seed: number,
	scatterDb = 0.55
): Float64Array {
	const next = rng(seed);
	const out = new Float64Array(f.length);
	for (let i = 0; i < f.length; i++) {
		const fi = f[i];
		const ripple = 0.45 * Math.sin(Math.log(fi) * 7.3 + (seed % 17)) + 0.25 * Math.sin(Math.log(fi) * 19.1);
		const notch = -2.6 * Math.exp(-(((Math.log(fi) - Math.log(615)) / 0.035) ** 2));
		const scatter = scatterDb * gauss(next);
		let p = specAt(spec, fi) * 10 ** ((ripple + notch + scatter) / 10);
		for (const m of channel.modes) p *= sdofT2(fi, m.fn, m.q);
		out[i] = p;
	}
	return out;
}

// ---------------------------------------------------------------------------------------------
// Modal: a simply supported rectangular plate

export interface PlateModel {
	nodes: Float32Array;
	triangles: Uint32Array;
	lines: Uint32Array;
	modes: { index: number; frequency_hz: number; damping: number; shape: Float32Array }[];
}

/**
 * Plate `a` x `b` (m), `nx` x `ny` elements, centred on the origin in the XY plane. Mode shapes
 * are sin(m pi x / a) sin(n pi y / b) out of plane; frequencies follow thin plate theory for
 * 3 mm aluminum, which puts the first mode near 66 Hz for the 0.6 x 0.4 m plate.
 */
export function plate(a: number, b: number, nx: number, ny: number, modeCount: number, seed: number): PlateModel {
	const cols = nx + 1;
	const rows = ny + 1;
	const nodes = new Float32Array(cols * rows * 3);
	for (let j = 0; j < rows; j++) {
		for (let i = 0; i < cols; i++) {
			const k = (j * cols + i) * 3;
			nodes[k] = (i / nx) * a - a / 2;
			nodes[k + 1] = (j / ny) * b - b / 2;
			nodes[k + 2] = 0;
		}
	}

	const triangles = new Uint32Array(nx * ny * 6);
	let t = 0;
	for (let j = 0; j < ny; j++) {
		for (let i = 0; i < nx; i++) {
			const n0 = j * cols + i;
			const n1 = n0 + 1;
			const n2 = n0 + cols + 1;
			const n3 = n0 + cols;
			// Alternate the diagonal so the mesh has no directional bias.
			if ((i + j) % 2 === 0) triangles.set([n0, n1, n2, n0, n2, n3], t);
			else triangles.set([n0, n1, n3, n1, n2, n3], t);
			t += 6;
		}
	}

	const perimeter: number[] = [];
	for (let i = 0; i < nx; i++) perimeter.push(i, i + 1, ny * cols + i, ny * cols + i + 1);
	for (let j = 0; j < ny; j++) perimeter.push(j * cols, (j + 1) * cols, j * cols + nx, (j + 1) * cols + nx);
	const lines = Uint32Array.from(perimeter);

	// Thin plate: f_mn = (pi / 2) sqrt(D / (rho h)) ((m / a)^2 + (n / b)^2)
	const E = 70e9;
	const nu = 0.33;
	const h = 0.003;
	const rho = 2700;
	const D = (E * h ** 3) / (12 * (1 - nu * nu));
	const c = (Math.PI / 2) * Math.sqrt(D / (rho * h));
	const pairs: { m: number; n: number; f: number }[] = [];
	for (let m = 1; m <= 8; m++) for (let n = 1; n <= 8; n++) pairs.push({ m, n, f: c * ((m / a) ** 2 + (n / b) ** 2) });
	pairs.sort((p, q) => p.f - q.f);

	const next = rng(seed);
	const modes = pairs.slice(0, modeCount).map((p, idx) => {
		const shape = new Float32Array(cols * rows * 3);
		for (let j = 0; j < rows; j++) {
			for (let i = 0; i < cols; i++) {
				const k = (j * cols + i) * 3;
				shape[k + 2] = Math.sin((p.m * Math.PI * i) / nx) * Math.sin((p.n * Math.PI * j) / ny);
			}
		}
		return {
			index: idx + 1,
			frequency_hz: Math.round(p.f * (1 + 0.01 * (next() - 0.5)) * 100) / 100,
			damping: Math.round((0.006 + 0.012 * next()) * 10000) / 10000,
			shape
		};
	});

	return { nodes, triangles, lines, modes };
}
