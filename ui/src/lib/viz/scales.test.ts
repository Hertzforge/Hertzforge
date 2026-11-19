import { describe, expect, it } from 'vitest';
import { bandClass, DEFAULT_BAND, errorPosition, errorRgb, lightness, oklchToRgb, sequentialRgb, type RowBand, type ThemeKind } from './scales';

const band: RowBand = { alarm_db: [3, 3], abort_db: [6, 6] };
const L = (db: number, theme: ThemeKind, b = band) => {
	const c = [0, 0, 0];
	errorRgb(db, b, theme, c);
	return lightness(c);
};
const Lseq = (v: number, theme: ThemeKind) => {
	const c = [0, 0, 0];
	sequentialRgb(v, theme, c);
	return lightness(c);
};
const THEMES: ThemeKind[] = ['light', 'dark'];

describe('color math', () => {
	it('OKLCH gray converts to the sRGB gray of the same lightness, and stays in gamut', () => {
		expect(oklchToRgb(1, 0, 0)).toEqual([255, 255, 255]);
		expect(oklchToRgb(0, 0, 0)).toEqual([0, 0, 0]);
		const g = oklchToRgb(0.6, 0, 0);
		expect(g[0]).toBe(g[1]);
		// Far too much chroma: reduced, never clipped per channel.
		for (const v of oklchToRgb(0.5, 0.5, 145)) expect(v).toBeGreaterThanOrEqual(0);
	});

	it('CIE lightness runs from 0 for black to 100 for white', () => {
		expect(lightness([0, 0, 0])).toBeCloseTo(0, 5);
		expect(lightness([255, 255, 255])).toBeCloseTo(100, 3);
		expect(lightness([119, 119, 119])).toBeCloseTo(50, 0);
	});
});

describe('the error scale is ordered by lightness (colorblind-safe)', () => {
	for (const theme of THEMES) {
		// Light theme: further from the reference is darker; dark theme: lighter. Either way the
		// distance from the background's lightness grows with the error.
		const away = (db: number) => (theme === 'light' ? 100 - L(db, theme) : L(db, theme));

		it(`${theme}: lightness moves away from the background monotonically with the size of the error, on both sides`, () => {
			for (const sign of [-1, 1]) {
				let prev = -Infinity;
				for (let m = 0; m <= 12; m += 0.25) {
					const a = away(sign * m);
					expect(a, `${sign * m} dB`).toBeGreaterThanOrEqual(prev - 0.5);
					prev = a;
				}
				// Across the whole scale the change is large: zero error and beyond abort are far apart.
				expect(away(sign * 12) - away(0)).toBeGreaterThan(50);
			}
		});

		it(`${theme}: lightness jumps at the alarm and the abort band edges`, () => {
			for (const sign of [-1, 1]) {
				const jumpAlarm = away(sign * 3.01) - away(sign * 3);
				const jumpAbort = away(sign * 6.01) - away(sign * 6);
				expect(jumpAlarm, 'alarm edge').toBeGreaterThan(8);
				expect(jumpAbort, 'abort edge').toBeGreaterThan(8);
			}
		});

		it(`${theme}: the three bands never overlap in lightness`, () => {
			const span = (lo: number, hi: number) => {
				const vs = [];
				for (let m = lo; m <= hi; m += 0.05) vs.push(away(m), away(-m));
				return [Math.min(...vs), Math.max(...vs)];
			};
			const inside = span(0, 3);
			const between = span(3.05, 6);
			const beyond = span(6.05, 12);
			expect(inside[1]).toBeLessThan(between[0]);
			expect(between[1]).toBeLessThan(beyond[0]);
		});

		it(`${theme}: the same size of error has the same lightness above and below the reference`, () => {
			for (const m of [0.5, 2, 3, 4, 6, 9, 12]) expect(Math.abs(L(m, theme) - L(-m, theme)), `${m} dB`).toBeLessThan(3);
		});

		it(`${theme}: zero error sits next to the plot's background`, () => {
			const bg = theme === 'light' ? 100 : lightness([12, 18, 24]);
			expect(Math.abs(L(0, theme) - bg)).toBeLessThan(15);
		});
	}

	it('the scale follows each row band, sides separately', () => {
		const lopsided: RowBand = { alarm_db: [1, 3], abort_db: [2, 6] };
		expect(bandClass(-1.5, lopsided)).toBe(1);
		expect(bandClass(1.5, lopsided)).toBe(0);
		expect(bandClass(-2.5, lopsided)).toBe(2);
		expect(bandClass(5, lopsided)).toBe(1);
		expect(errorPosition(-1, lopsided)).toEqual({ side: 0, cls: 0, t: 1 });
		expect(bandClass(3, DEFAULT_BAND)).toBe(0);
		expect(bandClass(3.0001, DEFAULT_BAND)).toBe(1);
	});

	it('a missing value is reported and drawn neutral', () => {
		const c = [9, 9, 9];
		expect(errorRgb(NaN, band, 'light', c)).toBe(false);
		expect(lightness(c)).toBeGreaterThan(95);
	});
});

describe('the coherence scale is one hue, ordered by lightness', () => {
	for (const theme of THEMES) {
		it(`${theme}: lightness is monotonic from 0 to 1 and covers a wide range`, () => {
			let prev = Lseq(0, theme);
			for (let v = 0.02; v <= 1.0001; v += 0.02) {
				const l = Lseq(v, theme);
				if (theme === 'light') expect(l).toBeLessThanOrEqual(prev + 0.3);
				else expect(l).toBeGreaterThanOrEqual(prev - 0.3);
				prev = l;
			}
			expect(Math.abs(Lseq(1, theme) - Lseq(0, theme))).toBeGreaterThan(55);
		});
	}
});
