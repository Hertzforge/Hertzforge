import { describe, expect, it } from 'vitest';
import { beforeEach } from 'vitest';
import { theme } from '$lib/theme/theme.svelte';
import { DARK_LOW, colormap, colormapGradient, colormapBytes, colormapLow } from './colormap';

// the whole map, as the light theme has it; the dark theme's part has its own tests
beforeEach(() => {
	theme.resolved = 'light';
});

/** Relative luminance (WCAG) of an sRGB color in 0..1. */
function luminance([r, g, b]: [number, number, number]): number {
	const lin = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
	return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

describe('colormap', () => {
	it('rises in lightness from end to end, so its order reads in grayscale', () => {
		let last = -1;
		for (let i = 0; i <= 100; i++) {
			const l = luminance(colormap(i / 100));
			expect(l).toBeGreaterThan(last);
			last = l;
		}
	});

	it('is viridis, end to end', () => {
		expect(colormap(0).map((c) => Math.round(c * 255))).toEqual([68, 1, 84]);
		expect(colormap(1).map((c) => Math.round(c * 255))).toEqual([253, 231, 37]);
		expect(colormapGradient()).toContain('rgb(68 1 84) 0.0%');
	});
});

describe('colormapBytes', () => {
	it('runs the colormap end to end as opaque sRGB bytes, for the shader to look up', () => {
		// a mode is colored per pixel from this texture, so a face runs the whole colormap
		// rather than a blend of its corners' colors (Brandon, 2026-11-14)
		const bytes = colormapBytes(256);
		expect(bytes.length).toBe(256 * 4);
		expect([...bytes.slice(0, 4)]).toEqual([68, 1, 84, 255]);
		expect([...bytes.slice(-4)]).toEqual([253, 231, 37, 255]);
		const middle = colormap(128 / 255).map((c) => Math.round(c * 255));
		expect([...bytes.slice(128 * 4, 128 * 4 + 3)]).toEqual(middle);
	});
});

describe('the colormap on the dark theme', () => {
	it('starts a fifth of the way in and is the same map from there', () => {
		theme.resolved = 'dark';
		expect(colormapLow()).toBe(DARK_LOW);
		const whole = (t: number) => colormap(t, [0, 0, 0], 0);
		expect(colormap(0)).toEqual(whole(DARK_LOW));
		expect(colormap(0.5)).toEqual(whole(DARK_LOW + 0.5 * (1 - DARK_LOW)));
		expect(colormap(1).map((c) => Math.round(c * 255))).toEqual([253, 231, 37]);
		// its first color is no longer the dark purple, on the bar and in the shader's copy alike
		expect(colormap(0).map((c) => Math.round(c * 255))).toEqual([65, 68, 135]);
		expect(colormapGradient()).toContain('rgb(65 68 135) 0.0%');
		expect([...colormapBytes(256).slice(0, 3)]).toEqual([65, 68, 135]);
	});

	it('still rises in lightness from end to end, and starts lighter than the whole map does', () => {
		theme.resolved = 'dark';
		let last = -1;
		for (let i = 0; i <= 100; i++) {
			const l = luminance(colormap(i / 100));
			expect(l).toBeGreaterThan(last);
			last = l;
		}
		expect(luminance(colormap(0))).toBeGreaterThan(3 * luminance(colormap(0, [0, 0, 0], 0)));
	});

	it('leaves the light theme the whole map', () => {
		theme.resolved = 'light';
		expect(colormapLow()).toBe(0);
		expect(colormap(0).map((c) => Math.round(c * 255))).toEqual([68, 1, 84]);
	});
});
