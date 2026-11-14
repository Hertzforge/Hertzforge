import { describe, expect, it } from 'vitest';
import { axesAt, dragScale, panFractions, wheelScale } from './mouseModel';
import { panBy, zoomAbout } from './paneOptions';

const area = { left: 50, top: 10, right: 450, bottom: 310 };

describe('the plot mouse model, after pyqtgraph', () => {
	it('moves both axes inside the area, and one alone over its axis', () => {
		expect(axesAt(200, 100, area)).toEqual({ x: true, y: true });
		expect(axesAt(20, 100, area)).toEqual({ x: false, y: true });
		expect(axesAt(200, 330, area)).toEqual({ x: true, y: false });
		expect(axesAt(20, 330, area)).toBeNull();
		expect(axesAt(200, 2, area)).toBeNull();
	});

	it('zooms in on a wheel turned up and out turned down, a notch to about 0.74', () => {
		expect(wheelScale(-100)).toBeCloseTo(1.02 ** -15, 6);
		expect(wheelScale(-100)).toBeLessThan(1);
		expect(wheelScale(100)).toBeGreaterThan(1);
		expect(wheelScale(-3, 1)).toBeCloseTo(wheelScale(-99), 6);
		expect(wheelScale(-1e6)).toBe(0.25);
	});

	it('zooms in on a right drag right or up, out left or down', () => {
		const [sx, sy] = dragScale(10, -10);
		expect(sx).toBeLessThan(1);
		expect(sy).toBeLessThan(1);
		const [ox, oy] = dragScale(-10, 10);
		expect(ox).toBeGreaterThan(1);
		expect(oy).toBeGreaterThan(1);
		// about the press: the value under it stays put
		expect(zoomAbout(0, 100, 30, sx, false)[0]).toBeCloseTo(30 - 30 * sx, 9);
	});

	it('pans so the data follows the pointer', () => {
		const [fx, fy] = panFractions(40, 30, 400, 300);
		expect(panBy(0, 100, fx, false)).toEqual([-10, 90]);
		expect(panBy(0, 100, fy, false)).toEqual([10, 110]);
	});
});
