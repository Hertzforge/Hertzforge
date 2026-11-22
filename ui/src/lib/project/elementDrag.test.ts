import { describe, expect, it } from 'vitest';
import { draggedElements, movesInto } from './elementDrag';

describe('dragging elements onto an element group', () => {
	it('carries the marked elements when the drag starts on one of them, else the one dragged', () => {
		expect(draggedElements([3, 7, 9], 7)).toEqual([3, 7, 9]);
		expect(draggedElements([3, 7, 9], 4)).toEqual([4]);
		expect(draggedElements([], 4)).toEqual([4]);
	});
	it('moves something only when some of them are not in that group already', () => {
		const groups = [
			{ id: 1, elements: [1, 2, 3] },
			{ id: 2, elements: [] }
		];
		expect(movesInto(groups, [2, 3], 2)).toBe(true);
		expect(movesInto(groups, [2, 3], 1)).toBe(false);
		expect(movesInto(groups, [3, 4], 1)).toBe(true);
	});
});
