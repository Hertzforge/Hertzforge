/**
 * Color scales for the MIMO run views, ordered by lightness so that every meaning survives in
 * grayscale and for every kind of color vision.
 *
 * The error scale is diverging with a neutral middle: an error of 0 dB is drawn close to the
 * plot's background, and the further a line is from its reference the further its color is from
 * the background in lightness, the same on both sides. Below the reference is a blue family and
 * above it an orange family, so the sign is told by hue and by the cursor's number, never the
 * size. The scale breaks at the row's alarm and abort bands: lightness jumps at each band edge,
 * so "inside alarm", "between alarm and abort" and "beyond abort" are three separate lightness
 * ranges, and the views add marks (hatching, symbols, words) for the abort band on top.
 *
 * The coherence scale is sequential in one hue: 0 is near the background, 1 the furthest from it.
 *
 * Colors are defined in OKLCH and converted to sRGB here; nothing is read from the page.
 */

export type ThemeKind = 'light' | 'dark';
export type Rgb = [number, number, number];

/** A row's alarm and abort bands, `[below, above]` in positive dB around the reference. */
export interface RowBand {
	alarm_db: [number, number];
	abort_db: [number, number];
}

/** The service's default bands when a setup states none. */
export const DEFAULT_BAND: RowBand = { alarm_db: [3, 3], abort_db: [6, 6] };

/** Where an error falls against its band: 0 inside alarm, 1 between alarm and abort, 2 beyond abort. */
export type BandClass = 0 | 1 | 2;

// ---- OKLCH to sRGB --------------------------------------------------------------------------

function linearToSrgb(c: number): number {
	return c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055;
}

function srgbToLinear(c: number): number {
	return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

/** Linear sRGB of an OKLCH color, possibly out of gamut. */
function oklchLinear(l: number, c: number, hDeg: number): Rgb {
	const h = (hDeg * Math.PI) / 180;
	const a = c * Math.cos(h);
	const b = c * Math.sin(h);
	const l_ = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
	const m_ = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
	const s_ = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;
	return [
		4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
		-1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
		-0.0041960863 * l_ - 0.7034186147 * m_ + 1.707614701 * s_
	];
}

const inGamut = (rgb: Rgb) => rgb.every((v) => v >= -1e-6 && v <= 1 + 1e-6);

/** sRGB bytes of an OKLCH color, chroma reduced until it is in gamut (lightness and hue kept). */
export function oklchToRgb(l: number, c: number, h: number): Rgb {
	let lo = 0;
	let hi = c;
	let rgb = oklchLinear(l, c, h);
	if (!inGamut(rgb)) {
		for (let i = 0; i < 24; i++) {
			const mid = (lo + hi) / 2;
			if (inGamut(oklchLinear(l, mid, h))) lo = mid;
			else hi = mid;
		}
		rgb = oklchLinear(l, lo, h);
	}
	return rgb.map((v) => Math.round(255 * linearToSrgb(Math.min(1, Math.max(0, v))))) as Rgb;
}

/** CIE L* (0 to 100) of sRGB bytes: the lightness a gray of the same luminance would have. */
export function lightness(rgb: ArrayLike<number>): number {
	const y = 0.2126 * srgbToLinear(rgb[0] / 255) + 0.7152 * srgbToLinear(rgb[1] / 255) + 0.0722 * srgbToLinear(rgb[2] / 255);
	const f = y > 216 / 24389 ? Math.cbrt(y) : (24389 / 27 * y + 16) / 116;
	return 116 * f - 16;
}

// ---- the error scale ------------------------------------------------------------------------

/** OKLCH lightness at the start and end of each band class: [inside alarm, alarm to abort, beyond abort]. */
const ERROR_L: Record<ThemeKind, [number, number][]> = {
	light: [
		[0.975, 0.8],
		[0.7, 0.58],
		[0.45, 0.33]
	],
	dark: [
		[0.215, 0.36],
		[0.48, 0.6],
		[0.75, 0.9]
	]
};
const ERROR_C: [number, number][] = [
	[0.004, 0.05],
	[0.09, 0.11],
	[0.12, 0.13]
];
/** Below the reference: blue. Above it: orange. */
const HUE_BELOW = 250;
const HUE_ABOVE = 58;
const STEPS = 48;

/** The error scale's lookup table: [side][class][step] as packed RGB bytes. */
function buildErrorLut(theme: ThemeKind): Uint8ClampedArray {
	const lut = new Uint8ClampedArray(2 * 3 * STEPS * 3);
	for (let side = 0; side < 2; side++) {
		for (let k = 0; k < 3; k++) {
			for (let s = 0; s < STEPS; s++) {
				const t = s / (STEPS - 1);
				const [l0, l1] = ERROR_L[theme][k];
				const [c0, c1] = ERROR_C[k];
				const rgb = oklchToRgb(l0 + (l1 - l0) * t, c0 + (c1 - c0) * t, side === 0 ? HUE_BELOW : HUE_ABOVE);
				lut.set(rgb, ((side * 3 + k) * STEPS + s) * 3);
			}
		}
	}
	return lut;
}

const errorLuts: Partial<Record<ThemeKind, Uint8ClampedArray>> = {};
function errorLut(theme: ThemeKind): Uint8ClampedArray {
	return (errorLuts[theme] ??= buildErrorLut(theme));
}

/** The band class of an error against a band. */
export function bandClass(db: number, band: RowBand): BandClass {
	const side = db < 0 ? 0 : 1;
	const m = Math.abs(db);
	if (m <= band.alarm_db[side]) return 0;
	if (m <= band.abort_db[side]) return 1;
	return 2;
}

/**
 * Where an error sits on the scale: its side (0 below, 1 above), its band class, and how far it
 * is through that class (0 to 1). Beyond abort runs to twice the abort band.
 */
export function errorPosition(db: number, band: RowBand): { side: 0 | 1; cls: BandClass; t: number } {
	const side: 0 | 1 = db < 0 ? 0 : 1;
	const m = Math.abs(db);
	const a = Math.max(1e-6, band.alarm_db[side]);
	const b = Math.max(a + 1e-6, band.abort_db[side]);
	if (m <= a) return { side, cls: 0, t: m / a };
	if (m <= b) return { side, cls: 1, t: (m - a) / (b - a) };
	return { side, cls: 2, t: Math.min(1, (m - b) / b) };
}

/**
 * The RGB bytes for an error in dB against its row's band, written at `out[at..at+2]`. A value
 * that is not a number is written as the background's neutral and reported false.
 */
export function errorRgb(db: number, band: RowBand, theme: ThemeKind, out: Uint8ClampedArray | number[], at = 0): boolean {
	const lut = errorLut(theme);
	if (!Number.isFinite(db)) {
		const k = 0;
		out[at] = lut[k];
		out[at + 1] = lut[k + 1];
		out[at + 2] = lut[k + 2];
		return false;
	}
	const { side, cls, t } = errorPosition(db, band);
	const k = ((side * 3 + cls) * STEPS + Math.round(t * (STEPS - 1))) * 3;
	out[at] = lut[k];
	out[at + 1] = lut[k + 1];
	out[at + 2] = lut[k + 2];
	return true;
}

/** The error scale's color for one value, as CSS. */
export function errorCss(db: number, band: RowBand, theme: ThemeKind): string {
	const c = [0, 0, 0];
	errorRgb(db, band, theme, c);
	return `rgb(${c[0]} ${c[1]} ${c[2]})`;
}

/**
 * The legend for the error scale over `-span..span` dB, as CSS gradient stops (sharp at the band
 * edges): what the color strip under a heatmap draws.
 */
export function errorGradient(band: RowBand, theme: ThemeKind, span = 2 * Math.max(band.abort_db[0], band.abort_db[1])): string {
	const stops: string[] = [];
	const n = 120;
	const edges = new Set([-band.abort_db[0], -band.alarm_db[0], band.alarm_db[1], band.abort_db[1]]);
	let prev: string | null = null;
	for (let i = 0; i <= n; i++) {
		const v = -span + (2 * span * i) / n;
		const pct = ((i / n) * 100).toFixed(2);
		const css = errorCss(v, band, theme);
		// A band edge between two stops: end the old color and start the new at the same place.
		for (const e of edges) {
			const before = -span + (2 * span * (i - 1)) / n;
			if (i > 0 && before < e && v >= e && prev) {
				const at = (((e + span) / (2 * span)) * 100).toFixed(2);
				stops.push(`${prev} ${at}%`, `${errorCss(e + (e > 0 ? 1e-6 : -1e-6), band, theme)} ${at}%`);
			}
		}
		stops.push(`${css} ${pct}%`);
		prev = css;
	}
	return `linear-gradient(to right, ${stops.join(', ')})`;
}

// ---- the coherence scale --------------------------------------------------------------------

const COH_L: Record<ThemeKind, [number, number]> = { light: [0.975, 0.33], dark: [0.215, 0.92] };
const COH_HUE = 250;
const cohLuts: Partial<Record<ThemeKind, Uint8ClampedArray>> = {};

function cohLut(theme: ThemeKind): Uint8ClampedArray {
	let lut = cohLuts[theme];
	if (lut) return lut;
	lut = new Uint8ClampedArray(64 * 3);
	const [l0, l1] = COH_L[theme];
	for (let s = 0; s < 64; s++) {
		const t = s / 63;
		lut.set(oklchToRgb(l0 + (l1 - l0) * t, 0.01 + 0.12 * t, COH_HUE), s * 3);
	}
	return (cohLuts[theme] = lut);
}

/** RGB bytes of a value from 0 to 1 on the sequential scale, at `out[at..at+2]`. */
export function sequentialRgb(v: number, theme: ThemeKind, out: Uint8ClampedArray | number[], at = 0): boolean {
	const lut = cohLut(theme);
	const ok = Number.isFinite(v);
	const k = ok ? Math.round(Math.min(1, Math.max(0, v)) * 63) * 3 : 0;
	out[at] = lut[k];
	out[at + 1] = lut[k + 1];
	out[at + 2] = lut[k + 2];
	return ok;
}

export function sequentialCss(v: number, theme: ThemeKind): string {
	const c = [0, 0, 0];
	sequentialRgb(v, theme, c);
	return `rgb(${c[0]} ${c[1]} ${c[2]})`;
}

export function sequentialGradient(theme: ThemeKind): string {
	const stops = Array.from({ length: 11 }, (_, i) => `${sequentialCss(i / 10, theme)} ${i * 10}%`);
	return `linear-gradient(to right, ${stops.join(', ')})`;
}

/** Words for a band class, for the cursor and the screen reader. */
export const BAND_WORDS: Record<BandClass, string> = {
	0: 'inside alarm',
	1: 'beyond alarm',
	2: 'beyond abort'
};
