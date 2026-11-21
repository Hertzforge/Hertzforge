import { describe, expect, it } from 'vitest';
import { gizmoReading, gridStep, stillFramed, idList, markedBy, markedRun, markedShapes, nearestEdge, pickClicks, setTriangles, modelPlane, nearestNode, numbered, placedPoint, toggledSurface } from './pick';

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

describe('a kept camera frames a model of about the same size', () => {
	it('keeps the camera for a move or a part picked, and fits again when the model grows or shrinks', () => {
		expect(stillFramed(0.87, 0.87)).toBe(true);
		expect(stillFramed(0.87, 1.0)).toBe(true);
		expect(stillFramed(0.87, 1.66)).toBe(false);
		expect(stillFramed(1.66, 0.87)).toBe(false);
		expect(stillFramed(0, 1)).toBe(false);
	});
});

describe('nodes marked by a click', () => {
	it('marks the one clicked, and with a modifier puts it in or takes it out', () => {

		expect(markedBy([4, 7], 9, false)).toEqual([9]);
		expect(markedBy([4, 7], 9, true)).toEqual([4, 7, 9]);
		expect(markedBy([4, 7], 7, true)).toEqual([4]);
	});

	it('clears on a click on nothing, unless a modifier is held', () => {

		expect(markedBy([4, 7], null, false)).toEqual([]);
		expect(markedBy([4, 7], null, true)).toEqual([4, 7]);
	});

	it('marks the run between two rows of the tree, either way round', () => {

		expect(markedRun([1, 2, 5, 8, 9], 8, 2)).toEqual([2, 5, 8]);
		expect(markedRun([1, 2, 5, 8, 9], 2, 8)).toEqual([2, 5, 8]);
		expect(markedRun([1, 2, 5], 77, 2)).toEqual([2]);
	});
});

describe('node numbers', () => {
	it('numbers the nodes in the picture and in front of the camera', () => {

		const screen = [10, 10, 500, 10, 20, 20, 30, 300];
		expect(numbered(screen, [true, true, false, true], 4, 100, 100)).toEqual([0]);
	});

	it('writes none when more are in the picture than can be read', () => {

		const screen = [10, 10, 20, 20, 30, 30];
		expect(numbered(screen, [true, true, true], 3, 100, 100, 2)).toBeNull();
		expect(numbered(screen, [true, true, true], 3, 100, 100, 3)).toEqual([0, 1, 2]);
	});
});

describe('markedShapes', () => {
	const sets = [
		{ link: 'nodes:G', ids: [1, 2], rows: [[0], [1]] },
		{ link: 'elements:G', ids: [7, 8, 9], faces: [[0, 1, 2], [2, 3, 4], [5, 6, 7]], edges: [[0, 1], [2, 3], []] }
	];
	it("fills and outlines the marked elements of each set, by its own link", () => {
		const marks: Record<string, number[]> = { 'elements:G': [9, 7], 'nodes:G': [1] };
		expect(markedShapes(sets, (link) => marks[link] ?? [])).toEqual({ faces: [0, 1, 2, 5, 6, 7], edges: [0, 1] });
	});
	it('draws nothing when nothing of its link is marked, or for a set with no faces or edges', () => {
		expect(markedShapes(sets, (link) => (link === 'nodes:G' ? [1, 2] : []))).toEqual({ faces: [], edges: [] });
		expect(markedShapes(sets, () => [42])).toEqual({ faces: [], edges: [] });
	});
});

describe('selecting elements in the view', () => {
	it('matches each triangle of the selectable faces to its element', () => {
		expect(setTriangles({ faces: [[0, 1, 2, 0, 2, 3], [], [4, 5, 6]] })).toEqual({ index: [0, 1, 2, 0, 2, 3, 4, 5, 6], owner: [0, 0, 2] });
	});
	it('picks the element whose side passes nearest on screen, within reach and in front', () => {
		// nodes 0 to 3 at the corners of a 100 px square; element 0 its top side, element 1 its right
		const screen = [0, 0, 100, 0, 100, 100, 0, 100];
		const set = { edges: [[0, 1], [1, 2]] };
		expect(nearestEdge(set, screen, [true, true, true, true], 50, 5)).toBe(0);
		expect(nearestEdge(set, screen, [true, true, true, true], 96, 60)).toBe(1);
		expect(nearestEdge(set, screen, [true, true, true, true], 50, 50)).toBeNull();
		expect(nearestEdge(set, screen, [true, false, true, true], 50, 5)).toBeNull();
		// past a side's end the distance is to its end, not to the line it lies on
		expect(nearestEdge({ edges: [[0, 1]] }, screen, [true, true, true, true], 130, 0)).toBeNull();
	});
	it("counts the clicks the chosen type takes, else the pick's own", () => {
		const pick = {
			count: 4,
			hint: 'Click 4 nodes to make a quad4',
			params: { type: 'quad4', group: 0 },
			settings: [
				{ param: 'type', options: [{ value: 'quad4', count: 4, hint: 'Click 4 nodes to make a quad4' }, { value: 'quad8', count: 8, hint: 'Click 8 nodes to make a quad8' }] },
				{ param: 'group', options: [{ value: 0 }, { value: 3 }] }
			]
		};
		expect(pickClicks(pick, {})).toEqual({ count: 4, hint: 'Click 4 nodes to make a quad4' });
		expect(pickClicks(pick, { type: 'quad8' })).toEqual({ count: 8, hint: 'Click 8 nodes to make a quad8' });
		expect(pickClicks({ count: 2, hint: 'h', params: {} }, { type: 'quad8' })).toEqual({ count: 2, hint: 'h' });
	});
});
