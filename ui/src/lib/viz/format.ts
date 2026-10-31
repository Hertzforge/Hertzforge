/** Number formatting for readouts and ticks. Display only: no numerics beyond rounding. */

const SUBNORMAL = 1e-5;

/** Compact, significant-figure formatting with no trailing zeros ("0.0412", "1250", "3.2e-6"). */
export function formatNumber(v: number, sig = 4): string {
	if (!Number.isFinite(v)) return v > 0 ? 'inf' : v < 0 ? '-inf' : '-';
	if (v === 0) return '0';
	const a = Math.abs(v);
	if (a < SUBNORMAL || a >= 1e7) return v.toExponential(Math.max(0, sig - 2)).replace('e+', 'e');
	const decimals = Math.max(0, sig - 1 - Math.floor(Math.log10(a)));
	return trimZeros(v.toFixed(Math.min(decimals, 8)));
}

/** Fixed significant figures, keeping trailing zeros, for readouts that must not jitter in width. */
export function formatFixed(v: number, sig = 4): string {
	if (!Number.isFinite(v)) return '-';
	if (v === 0) return '0';
	const a = Math.abs(v);
	if (a < SUBNORMAL || a >= 1e7) return v.toExponential(Math.max(0, sig - 1)).replace('e+', 'e');
	const decimals = Math.max(0, sig - 1 - Math.floor(Math.log10(a)));
	return v.toFixed(Math.min(decimals, 8));
}

/** Tick label: plain decimals between 1e-5 and 1e6, exponent form outside. */
export function formatTick(v: number): string {
	if (v === 0) return '0';
	const a = Math.abs(v);
	if (a < SUBNORMAL || a >= 1e6) {
		const e = Math.floor(Math.log10(a));
		const m = v / 10 ** e;
		return Math.abs(Math.abs(m) - 1) < 1e-9 ? `${m < 0 ? '-' : ''}1e${e}` : `${trimZeros(m.toFixed(1))}e${e}`;
	}
	if (a >= 10000) return v.toFixed(0).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
	if (a >= 1000) return v.toFixed(0);
	return String(Number(v.toPrecision(6)));
}

export function trimZeros(s: string): string {
	return s.includes('.') ? s.replace(/\.?0+$/, '') : s;
}

/** Thousands separators for counts ("16,261"). */
export function formatCount(n: number): string {
	return Math.round(n).toLocaleString('en-US');
}

export function withUnit(text: string, unit: string | null | undefined): string {
	return unit ? `${text} ${unit}` : text;
}
