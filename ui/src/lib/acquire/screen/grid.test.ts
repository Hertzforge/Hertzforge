/**
 * The page grid: placing, moving, resizing and tiling panels on a grid of equal cells.
 */
import { describe, expect, it } from 'vitest';
import {
	arrangeEvenly,
	clampCell,
	collisions,
	compact,
	firstFree,
	fits,
	grow,
	largestFree,
	moveTo,
	overlaps,
	rescale,
	resize,
	step,
	tiling,
	type Placed
} from './grid';

const P = (id: string, x: number, y: number, w: number, h: number): Placed => ({ id, at: { x, y, w, h } });
/** Positions by id, for comparing whole pages. */
const where = (ps: Placed[] | null) => (ps ? Object.fromEntries(ps.map((p) => [p.id, [p.at.x, p.at.y, p.at.w, p.at.h]])) : null);

describe('collisions', () => {
	it('tells overlapping rectangles from ones that only touch', () => {
		expect(overlaps({ x: 0, y: 0, w: 6, h: 6 }, { x: 5, y: 5, w: 2, h: 2 })).toBe(true);
		expect(overlaps({ x: 0, y: 0, w: 6, h: 6 }, { x: 6, y: 0, w: 6, h: 6 })).toBe(false);
		expect(overlaps({ x: 0, y: 0, w: 6, h: 6 }, { x: 0, y: 6, w: 6, h: 6 })).toBe(false);
		const page = [P('a', 0, 0, 6, 6), P('b', 6, 0, 6, 6), P('c', 0, 6, 12, 6)];
		expect(collisions(page, { x: 4, y: 4, w: 4, h: 4 }).map((p) => p.id)).toEqual(['a', 'b', 'c']);
		expect(collisions(page, { x: 4, y: 4, w: 4, h: 4 }, 'a').map((p) => p.id)).toEqual(['b', 'c']);
		expect(fits(page, 12, 12)).toBe(true);
		expect(fits([...page, P('d', 11, 11, 2, 1)], 12, 12)).toBe(false);
		expect(fits([P('a', 0, 0, 6, 6), P('b', 5, 0, 6, 6)], 12, 12)).toBe(false);
	});

	it('keeps a rectangle whole and on the grid', () => {
		expect(clampCell({ x: 10, y: -2, w: 4, h: 0 }, 12, 12)).toEqual({ x: 8, y: 0, w: 4, h: 1 });
		expect(clampCell({ x: 0.4, y: 3.6, w: 20, h: 2.2 }, 12, 12)).toEqual({ x: 0, y: 4, w: 12, h: 2 });
		expect(clampCell({ x: Number.NaN, y: 0, w: 1, h: 1 }, 12, 12, 2)).toEqual({ x: 0, y: 0, w: 2, h: 2 });
	});
});

describe('free space', () => {
	it('finds the first place a panel fits, in reading order', () => {
		const page = [P('a', 0, 0, 6, 6), P('b', 6, 0, 6, 3)];
		expect(firstFree(page, 6, 3, 12, 12)).toEqual({ x: 6, y: 3, w: 6, h: 3 });
		expect(firstFree(page, 6, 6, 12, 12)).toEqual({ x: 6, y: 3, w: 6, h: 6 });
		expect(firstFree(page, 7, 6, 12, 12)).toEqual({ x: 0, y: 6, w: 7, h: 6 });
		expect(firstFree([P('a', 0, 0, 12, 12)], 1, 1, 12, 12)).toBeNull();
		expect(firstFree([], 13, 1, 12, 12)).toBeNull();
	});

	it('finds the largest empty rectangle', () => {
		expect(largestFree([], 12, 12)).toEqual({ x: 0, y: 0, w: 12, h: 12 });
		expect(largestFree([P('a', 0, 0, 12, 6)], 12, 12)).toEqual({ x: 0, y: 6, w: 12, h: 6 });
		expect(largestFree([P('a', 0, 0, 6, 12), P('b', 6, 0, 6, 8)], 12, 12)).toEqual({ x: 6, y: 8, w: 6, h: 4 });
		expect(largestFree([P('a', 0, 0, 12, 11)], 12, 12, 3, 3)).toBeNull();
	});
});

describe('moving a panel', () => {
	it('pushes the panels in its way down', () => {
		// Three rows of four; the bottom one moved to the top pushes the others down a row each.
		const page = [P('a', 0, 0, 12, 4), P('b', 0, 4, 12, 4), P('c', 0, 8, 12, 4)];
		expect(where(moveTo(page, 'c', 0, 0, 12, 12))).toEqual({ a: [0, 4, 12, 4], b: [0, 8, 12, 4], c: [0, 0, 12, 4] });
		// A small panel dropped onto the top-left of a column pushes the column down.
		const col = [P('a', 0, 0, 6, 4), P('b', 0, 4, 6, 4), P('c', 6, 0, 6, 4)];
		expect(where(moveTo(col, 'c', 0, 0, 12, 12))).toEqual({ a: [0, 4, 6, 4], b: [0, 8, 6, 4], c: [0, 0, 6, 4] });
	});

	it('closes the gaps it leaves, upward', () => {
		const page = [P('a', 0, 0, 6, 4), P('b', 0, 4, 6, 4), P('c', 6, 0, 6, 4)];
		// a goes right, under c; b rises to the top of the left column.
		expect(where(moveTo(page, 'a', 6, 4, 12, 12))).toEqual({ a: [6, 4, 6, 4], b: [0, 0, 6, 4], c: [6, 0, 6, 4] });
		// Dropped low in an empty column, a panel rises to its top.
		expect(where(moveTo(page, 'c', 6, 7, 12, 12))).toEqual({ a: [0, 0, 6, 4], b: [0, 4, 6, 4], c: [6, 0, 6, 4] });
	});

	it('swaps two panels of a size when the page has no room below', () => {
		const side = [P('a', 0, 0, 6, 12), P('b', 6, 0, 6, 12)];
		expect(where(moveTo(side, 'a', 6, 0, 12, 12))).toEqual({ a: [6, 0, 6, 12], b: [0, 0, 6, 12] });
		const quad = [P('a', 0, 0, 6, 6), P('b', 6, 0, 6, 6), P('c', 0, 6, 6, 6), P('d', 6, 6, 6, 6)];
		expect(where(moveTo(quad, 'a', 6, 6, 12, 12))).toEqual({ a: [6, 6, 6, 6], b: [6, 0, 6, 6], c: [0, 6, 6, 6], d: [0, 0, 6, 6] });
	});

	it('refuses a move with no room for what it displaces, and leaves the page as it was', () => {
		const quad = [P('a', 0, 0, 6, 6), P('b', 6, 0, 6, 6), P('c', 0, 6, 6, 6), P('d', 6, 6, 6, 6)];
		const before = where(quad);
		expect(moveTo(quad, 'a', 3, 0, 12, 12)).toBeNull();
		expect(where(quad)).toEqual(before);
		expect(moveTo(quad, 'zz', 0, 0, 12, 12)).toBeNull();
	});

	it('keeps a moved panel on the grid', () => {
		const page = [P('a', 0, 0, 4, 4)];
		expect(where(moveTo(page, 'a', 20, 20, 12, 12))).toEqual({ a: [8, 0, 4, 4] });
	});
});

describe('resizing a panel', () => {
	it('clamps to the grid and pushes what it grows over down', () => {
		const page = [P('a', 0, 0, 6, 4), P('b', 0, 4, 6, 4)];
		expect(where(resize(page, 'a', 30, 6, 12, 12))).toEqual({ a: [0, 0, 12, 6], b: [0, 6, 6, 4] });
		expect(where(resize(page, 'a', 0, 0, 12, 12, 2))).toEqual({ a: [0, 0, 2, 2], b: [0, 2, 6, 4] });
	});

	it('refuses to grow over a neighbour that has nowhere to go', () => {
		const side = [P('a', 0, 0, 6, 12), P('b', 6, 0, 6, 12)];
		expect(resize(side, 'a', 7, 12, 12, 12)).toBeNull();
	});
});

describe('compacting', () => {
	it('moves every panel up as far as it goes, and leaves a pinned one where it is', () => {
		const page = [P('a', 0, 3, 6, 3), P('b', 0, 9, 6, 3), P('c', 6, 5, 6, 2)];
		expect(where(compact(page))).toEqual({ a: [0, 0, 6, 3], b: [0, 3, 6, 3], c: [6, 0, 6, 2] });
		expect(where(compact(page, 'a'))).toEqual({ a: [0, 3, 6, 3], b: [0, 6, 6, 3], c: [6, 0, 6, 2] });
	});
});

describe('the keyboard', () => {
	it('moves a panel by a cell, and past a neighbour to swap when a cell will not do', () => {
		const page = [P('a', 0, 0, 4, 4), P('b', 4, 0, 4, 4)];
		expect(where(step(page, 'b', 1, 0, 12, 12))).toEqual({ a: [0, 0, 4, 4], b: [5, 0, 4, 4] });
		const stacked = [P('a', 0, 0, 12, 6), P('b', 0, 6, 12, 6)];
		expect(where(step(stacked, 'a', 0, 1, 12, 12))).toEqual({ a: [0, 6, 12, 6], b: [0, 0, 12, 6] });
		expect(where(step(stacked, 'b', 0, -1, 12, 12))).toEqual({ a: [0, 6, 12, 6], b: [0, 0, 12, 6] });
		const side = [P('a', 0, 0, 6, 12), P('b', 6, 0, 6, 12)];
		expect(where(step(side, 'a', 1, 0, 12, 12))).toEqual({ a: [6, 0, 6, 12], b: [0, 0, 6, 12] });
		// Nowhere to go: at the edge, or down an empty column the page closes again.
		expect(step(side, 'b', 1, 0, 12, 12)).toBeNull();
		expect(step([P('a', 0, 0, 4, 4)], 'a', 0, 1, 12, 12)).toBeNull();
	});

	it('resizes a panel by a cell, never below the least size or past the edge', () => {
		const page = [P('a', 0, 0, 4, 4), P('b', 0, 4, 4, 4)];
		expect(where(grow(page, 'a', 1, 0, 12, 12))).toEqual({ a: [0, 0, 5, 4], b: [0, 4, 4, 4] });
		expect(where(grow(page, 'a', 0, 1, 12, 12))).toEqual({ a: [0, 0, 4, 5], b: [0, 5, 4, 4] });
		expect(where(grow(page, 'a', 0, -1, 12, 12))).toEqual({ a: [0, 0, 4, 3], b: [0, 3, 4, 4] });
		expect(grow(page, 'a', -3, 0, 12, 12, 2)).toBeNull();
		expect(grow([P('a', 0, 0, 12, 4)], 'a', 1, 0, 12, 12)).toBeNull();
	});
});

describe('arranging evenly', () => {
	it('fills the page with one panel', () => {
		expect(tiling(1, 12, 12, 1.5)).toEqual([{ x: 0, y: 0, w: 12, h: 12 }]);
	});

	it('puts two side by side on a wide page and stacks them on a tall one', () => {
		expect(tiling(2, 12, 12, 4)).toEqual([
			{ x: 0, y: 0, w: 6, h: 12 },
			{ x: 6, y: 0, w: 6, h: 12 }
		]);
		expect(tiling(2, 12, 12, 0.8)).toEqual([
			{ x: 0, y: 0, w: 12, h: 6 },
			{ x: 0, y: 6, w: 12, h: 6 }
		]);
	});

	it('puts four in two rows of two, and three as two above one', () => {
		expect(tiling(4, 12, 12, 1.6)).toEqual([
			{ x: 0, y: 0, w: 6, h: 6 },
			{ x: 6, y: 0, w: 6, h: 6 },
			{ x: 0, y: 6, w: 6, h: 6 },
			{ x: 6, y: 6, w: 6, h: 6 }
		]);
		expect(tiling(3, 12, 12, 1.4)).toEqual([
			{ x: 0, y: 0, w: 6, h: 6 },
			{ x: 6, y: 0, w: 6, h: 6 },
			{ x: 0, y: 6, w: 12, h: 6 }
		]);
	});

	it('always fills the page, with whole cells as equal as the grid allows', () => {
		for (const n of [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]) {
			for (const aspect of [0.6, 1, 1.4, 2.2, 3.5]) {
				const cells = tiling(n, 12, 12, aspect);
				const page = cells.map((at, i) => ({ id: String(i), at }));
				expect(cells).toHaveLength(n);
				expect(fits(page, 12, 12)).toBe(true);
				expect(cells.reduce((s, c) => s + c.w * c.h, 0)).toBe(144);
			}
		}
		expect(tiling(5, 2, 2, 1)).toEqual([]);
	});

	it('keeps the panels in reading order', () => {
		const page = [P('late', 6, 6, 3, 3), P('early', 0, 0, 3, 3)];
		expect(where(arrangeEvenly(page, 12, 12, 0.8))).toEqual({ early: [0, 0, 12, 6], late: [0, 6, 12, 6] });
	});
});

describe('a grid of another size', () => {
	it('keeps each panel its share of the page', () => {
		const page = [P('a', 0, 0, 6, 12), P('b', 6, 0, 6, 6), P('c', 6, 6, 6, 6)];
		expect(where(rescale(page, { cols: 12, rows: 12 }, { cols: 24, rows: 24 }, 1.5))).toEqual({ a: [0, 0, 12, 24], b: [12, 0, 12, 12], c: [12, 12, 12, 12] });
		expect(where(rescale(page, { cols: 12, rows: 12 }, { cols: 6, rows: 6 }, 1.5))).toEqual({ a: [0, 0, 3, 6], b: [3, 0, 3, 3], c: [3, 3, 3, 3] });
	});

	it('tiles evenly when the coarser grid cannot hold the panels as they were', () => {
		// Three single cells side by side do not halve into whole cells.
		const page = [P('a', 0, 0, 1, 1), P('b', 1, 0, 1, 1), P('c', 2, 0, 1, 1)];
		const out = rescale(page, { cols: 12, rows: 12 }, { cols: 6, rows: 6 }, 0.8);
		expect(fits(out, 6, 6)).toBe(true);
		expect(where(out)).toEqual({ a: [0, 0, 6, 2], b: [0, 2, 6, 2], c: [0, 4, 6, 2] });
	});
});
