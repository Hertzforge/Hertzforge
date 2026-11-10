import { describe, expect, it } from 'vitest';
import { gizmoReading, gridStep, idList, modelPlane, nearestNode, placedPoint, toggledSurface } from './pick';

describe('nearestNode', () => {
	const screen = [0, 0, 10, 0, 100, 100];
	it('takes the nearest node within reach, of the pickable ones in front', () => {
		expect(nearestNode(screen, [true, true, true], 3, 8, 1)).toBe(1);
		expect(nearestNode(screen, [true, false, true], 3, 8, 1)).toBe(0);
		expect(nearestNode(screen, [true, true, true], 3, 60, 60)).toBeNull();
		expect(nearestNode(screen, [true, true, true], 2, 99, 99)).toBeNull();
	});
});

describe('placing a point', () => {
	const plate = [0, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 0];
	it('finds a flat model\'s plane, and none for a solid', () => {
		const plane = modelPlane(plate, 4)!;
		expect(Math.abs(plane.normal[2])).toBeCloseTo(1);
		expect(modelPlane([...plate, 0.5, 0.5, 0.3], 5)).toBeNull();
		expect(modelPlane([0, 0, 0, 1, 0, 0, 2, 0, 0], 3)).toBeNull();
	});

	it('lands on the plane of a flat model', () => {
		const p = placedPoint([0.3, 0.4, 5], [0, 0, -1], [0, 0, 0], modelPlane(plate, 4));
		expect(p[0]).toBeCloseTo(0.3);
		expect(p[1]).toBeCloseTo(0.4);
		expect(p[2]).toBeCloseTo(0);
	});

	it('lands at the depth of the nearest node otherwise, or when the ray runs along the plane', () => {
		const p = placedPoint([0, 0, 0], [1, 0, 0], [2, 3, 0], null);
		expect(p).toEqual([2, 0, 0]);
		const edgeOn = placedPoint([0, 0, 0], [1, 0, 0], [2, 3, 0], modelPlane(plate, 4));
		expect(edgeOn).toEqual([2, 0, 0]);
	});
});

describe('toggledSurface', () => {
	it('leaves out a surface in use and puts back one that is not', () => {
		expect(toggledSurface({ leave_out: '', put_back: '' }, [1, 2], 'leave_out', 'put_back', 2)).toEqual({ leave_out: '2', put_back: '' });
		expect(toggledSurface({ leave_out: '2, 5', put_back: '' }, [1], 'leave_out', 'put_back', 2)).toEqual({ put_back: '2', leave_out: '5' });
		expect(toggledSurface({ leave_out: '', put_back: '3' }, [1, 3], 'leave_out', 'put_back', 3)).toEqual({ leave_out: '3', put_back: '' });
	});

	it('writes ids as the service reads them', () => {
		expect(idList([4, 7, 9])).toBe('4, 7, 9');
	});
});

describe('the grid an arrow lands on', () => {
	it('is a tenth of an inch or a centimeter, in the unit shown', () => {
		expect(gridStep('in')).toBeCloseTo(0.1);
		expect(gridStep('ft')).toBeCloseTo(0.1 / 12);
		expect(gridStep('mm')).toBeCloseTo(10);
		expect(gridStep('m')).toBeCloseTo(0.01);
	});
	it('does not round the numbers of an undefined geometry', () => {
		expect(gridStep('undefined')).toBeNull();
	});
});

describe('the gimbal says how far a drag has gone', () => {
	it('turns in degrees and moves in the view\'s unit, to its grid', () => {
		expect(gizmoReading('rotate', 'z', 37, 'in')).toBe('Turn about Z: 37°');
		expect(gizmoReading('rotate', 'x', -12.5, 'in')).toBe('Turn about X: -12.5°');
		expect(gizmoReading('translate', 'x', 0.3, 'in')).toBe('Move along X: 0.3 in');
		expect(gizmoReading('translate', 'y', 0.25, 'm')).toBe('Move along Y: 0.25 m');
	});
});
