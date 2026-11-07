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
