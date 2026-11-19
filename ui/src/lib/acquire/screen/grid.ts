/**
 * The page grid: panels as rectangles of whole cells on a grid of equal cells, and the moves a
 * screen makes with them. Pure: every function returns new positions and changes nothing it is
 * given; a move that cannot be made returns null and the page stays as it was.
 *
 * The rules, in the order a move applies them: the panel goes where it was put; panels in the way
 * move down, in reading order, below whatever they meet; the page then closes its gaps upward. On
 * a page that would then run past its last row, the panels the moved one landed on take the first
 * free places instead (two panels of a size swap); when there is no room for them, the move is
 * refused.
 */

import type { Cell } from './types';

/** Anything placed on the grid: a panel, or a test's stand-in for one. */
export interface Placed {
	id: string;
	at: Cell;
}

/** The shape of a tile Arrange Evenly aims for, width over height: a spectrum's usual shape. */
export const IDEAL_TILE = 1.8;
/** What a last row with fewer, wider tiles costs against a full one, in the same measure. */
const UNEVEN = 0.3;

export function overlaps(a: Cell, b: Cell): boolean {
	return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

export function inside(at: Cell, cols: number, rows: number): boolean {
	return at.x >= 0 && at.y >= 0 && at.w >= 1 && at.h >= 1 && at.x + at.w <= cols && at.y + at.h <= rows;
}

export function sameCell(a: Cell, b: Cell): boolean {
	return a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h;
}

/** The panels `at` overlaps, leaving out `except`. */
export function collisions<T extends Placed>(panels: T[], at: Cell, except?: string): T[] {
	return panels.filter((p) => p.id !== except && overlaps(p.at, at));
}

/** `at` made whole and kept on the grid: at least `min` cells each way, no larger than the grid. */
export function clampCell(at: Cell, cols: number, rows: number, min = 1): Cell {
	const whole = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, Math.round(Number.isFinite(v) ? v : lo)));
	const w = whole(at.w, Math.min(min, cols), cols);
	const h = whole(at.h, Math.min(min, rows), rows);
	return { x: whole(at.x, 0, cols - w), y: whole(at.y, 0, rows - h), w, h };
}

/** Every panel on the grid and none over another. */
export function fits(panels: Placed[], cols: number, rows: number): boolean {
	for (let i = 0; i < panels.length; i++) {
		if (!inside(panels[i].at, cols, rows)) return false;
		for (let j = i + 1; j < panels.length; j++) if (overlaps(panels[i].at, panels[j].at)) return false;
	}
	return true;
}

/** Top to bottom, then left to right. */
export function readingOrder<T extends Placed>(panels: T[]): T[] {
	return [...panels].sort((a, b) => a.at.y - b.at.y || a.at.x - b.at.x || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}

/** The first place, in reading order, where a `w` by `h` panel fits; null when none. */
export function firstFree(panels: Placed[], w: number, h: number, cols: number, rows: number): Cell | null {
	if (w < 1 || h < 1 || w > cols || h > rows) return null;
	for (let y = 0; y + h <= rows; y++) {
		for (let x = 0; x + w <= cols; x++) {
			const at = { x, y, w, h };
			if (!panels.some((p) => overlaps(p.at, at))) return at;
		}
	}
	return null;
}

/**
 * The largest empty rectangle at least `minW` by `minH`, the first in reading order of those as
 * large; null when there is none.
 */
export function largestFree(panels: Placed[], cols: number, rows: number, minW = 1, minH = 1): Cell | null {
	// Filled cells, summed so any rectangle's count is four lookups.
	const sum: number[][] = Array.from({ length: rows + 1 }, () => new Array<number>(cols + 1).fill(0));
	for (let y = 0; y < rows; y++) {
		for (let x = 0; x < cols; x++) {
			const full = panels.some((p) => x >= p.at.x && x < p.at.x + p.at.w && y >= p.at.y && y < p.at.y + p.at.h) ? 1 : 0;
			sum[y + 1][x + 1] = full + sum[y][x + 1] + sum[y + 1][x] - sum[y][x];
		}
	}
	const filled = (x: number, y: number, w: number, h: number) => sum[y + h][x + w] - sum[y][x + w] - sum[y + h][x] + sum[y][x];
	let best: Cell | null = null;
	for (let y = 0; y < rows; y++) {
		for (let x = 0; x < cols; x++) {
			if (filled(x, y, 1, 1)) continue;
			for (let h = minH; y + h <= rows; h++) {
				for (let w = minW; x + w <= cols; w++) {
					if (filled(x, y, w, h)) break;
					if (!best || w * h > best.w * best.h) best = { x, y, w, h };
				}
			}
		}
	}
	return best;
}

/** Each panel moved up as far as it goes, in reading order; `pinned` stays where it is. */
export function compact<T extends Placed>(panels: T[], pinned?: string): T[] {
	const placed: T[] = panels.filter((p) => p.id === pinned);
	for (const p of readingOrder(panels.filter((q) => q.id !== pinned))) {
		let y = p.at.y;
		while (y > 0 && !placed.some((q) => overlaps(q.at, { ...p.at, y: y - 1 }))) y--;
		placed.push(y === p.at.y ? p : { ...p, at: { ...p.at, y } });
	}
	return inOrderOf(panels, placed);
}

function inOrderOf<T extends Placed>(order: T[], placed: T[]): T[] {
	const by = new Map(placed.map((p) => [p.id, p]));
	return order.map((p) => by.get(p.id) as T);
}

/** `id` placed at `target` (already on the grid), with the rest settled around it; null when there is no room. */
function settle<T extends Placed>(panels: T[], id: string, target: Cell, cols: number, rows: number): T[] | null {
	const me = panels.find((p) => p.id === id);
	if (!me) return null;
	const moved = { ...me, at: target };
	const others = readingOrder(panels.filter((p) => p.id !== id));

	// Panels in the way move down, below whatever they meet.
	const pushed: T[] = [moved];
	for (const p of others) {
		let at = p.at;
		for (let hit = collisions(pushed, at); hit.length; hit = collisions(pushed, at)) {
			at = { ...at, y: Math.max(...hit.map((h) => h.at.y + h.at.h)) };
		}
		pushed.push(at === p.at ? p : { ...p, at });
	}
	let out = compact(pushed, id);
	if (!fits(out, cols, rows)) {
		// The page has no room below: the panels the moved one landed on take the first free places.
		const grid: T[] = [moved, ...others.filter((p) => !overlaps(p.at, target))];
		for (const d of others.filter((p) => overlaps(p.at, target))) {
			const at = firstFree(grid, d.at.w, d.at.h, cols, rows);
			if (!at) return null;
			grid.push({ ...d, at });
		}
		out = compact(grid, id);
		if (!fits(out, cols, rows)) return null;
	}
	// And the page closes its gaps upward, the moved panel too.
	return inOrderOf(panels, compact(out));
}

/** Move `id` so its top-left cell is (`x`, `y`). */
export function moveTo<T extends Placed>(panels: T[], id: string, x: number, y: number, cols: number, rows: number): T[] | null {
	const me = panels.find((p) => p.id === id);
	if (!me) return null;
	return settle(panels, id, clampCell({ ...me.at, x, y }, cols, rows), cols, rows);
}

/** Make `id` `w` by `h` cells, its top-left cell where it is; at least `min` cells each way. */
export function resize<T extends Placed>(panels: T[], id: string, w: number, h: number, cols: number, rows: number, min = 1): T[] | null {
	const me = panels.find((p) => p.id === id);
	if (!me) return null;
	const at = clampCell({ x: me.at.x, y: me.at.y, w: Math.min(w, cols - me.at.x), h: Math.min(h, rows - me.at.y) }, cols, rows, min);
	return settle(panels, id, at, cols, rows);
}

/**
 * Move `id` one cell along (`dx`, `dy`), for the keyboard. Where one cell changes nothing (the
 * page closes the gap again) or there is no room, it goes further along, as far as the edge, to
 * the first place that works: past a neighbour, the two swap.
 */
export function step<T extends Placed>(panels: T[], id: string, dx: number, dy: number, cols: number, rows: number): T[] | null {
	const me = panels.find((p) => p.id === id);
	if (!me || (dx === 0 && dy === 0)) return null;
	let last = me.at;
	for (let k = 1; k <= Math.max(cols, rows); k++) {
		const at = clampCell({ ...me.at, x: me.at.x + dx * k, y: me.at.y + dy * k }, cols, rows);
		if (sameCell(at, last)) break;
		last = at;
		const out = settle(panels, id, at, cols, rows);
		if (out && !samePlaces(out, panels)) return out;
	}
	return null;
}

/** Grow or shrink `id` by (`dw`, `dh`) cells, for the keyboard; null when it cannot. */
export function grow<T extends Placed>(panels: T[], id: string, dw: number, dh: number, cols: number, rows: number, min = 1): T[] | null {
	const me = panels.find((p) => p.id === id);
	if (!me) return null;
	const w = me.at.w + dw;
	const h = me.at.h + dh;
	if (w < min || h < min || me.at.x + w > cols || me.at.y + h > rows) return null;
	const out = resize(panels, id, w, h, cols, rows, min);
	return out && !samePlaces(out, panels) ? out : null;
}

export function samePlaces(a: Placed[], b: Placed[]): boolean {
	if (a.length !== b.length) return false;
	const by = new Map(b.map((p) => [p.id, p.at]));
	return a.every((p) => {
		const q = by.get(p.id);
		return !!q && sameCell(p.at, q);
	});
}

/** `total` cells in `parts` runs as equal as whole cells allow, the larger first: [start, size] each. */
function split(total: number, parts: number): [number, number][] {
	const base = Math.floor(total / parts);
	const extra = total - base * parts;
	const out: [number, number][] = [];
	let at = 0;
	for (let i = 0; i < parts; i++) {
		const size = base + (i < extra ? 1 : 0);
		out.push([at, size]);
		at += size;
	}
	return out;
}

/**
 * `n` equal tiles filling a `cols` by `rows` grid on a page `aspect` wide for each unit of height,
 * in reading order: equal rows, equal tiles in a row, and a last row of fewer, wider tiles when
 * `n` does not fill it. Of the ways to do that, the one whose tiles come nearest the shape of a
 * spectrum plot (`IDEAL_TILE`), a full last row preferred. Empty when there is no room for `n`.
 */
export function tiling(n: number, cols: number, rows: number, aspect: number): Cell[] {
	if (n < 1) return [];
	const a = Number.isFinite(aspect) && aspect > 0 ? aspect : IDEAL_TILE;
	let best: { c: number; r: number; score: number } | null = null;
	for (let c = 1; c <= Math.min(n, cols); c++) {
		const r = Math.ceil(n / c);
		if (r > rows) continue;
		const last = n - (r - 1) * c;
		const off = (tile: number) => Math.abs(Math.log(tile / IDEAL_TILE));
		const score = ((n - last) * off((a * r) / c) + last * off((a * r) / last)) / n + (last < c ? UNEVEN : 0);
		if (!best || score < best.score - 1e-9) best = { c, r, score };
	}
	if (!best) return [];
	const out: Cell[] = [];
	const heights = split(rows, best.r);
	for (let i = 0; i < best.r; i++) {
		const inRow = i < best.r - 1 ? best.c : n - (best.r - 1) * best.c;
		for (const [x, w] of split(cols, inRow)) out.push({ x, y: heights[i][0], w, h: heights[i][1] });
	}
	return out;
}

/** The panels tiled evenly over the page, keeping their reading order. */
export function arrangeEvenly<T extends Placed>(panels: T[], cols: number, rows: number, aspect: number): T[] {
	const cells = tiling(panels.length, cols, rows, aspect);
	if (cells.length !== panels.length) return panels;
	const order = readingOrder(panels).map((p) => p.id);
	return panels.map((p) => ({ ...p, at: cells[order.indexOf(p.id)] }));
}

/**
 * The panels on a grid of another size, each keeping its share of the page; tiled evenly when
 * the new grid cannot hold them as they were.
 */
export function rescale<T extends Placed>(panels: T[], from: { cols: number; rows: number }, to: { cols: number; rows: number }, aspect: number): T[] {
	const kx = to.cols / from.cols;
	const ky = to.rows / from.rows;
	const out = panels.map((p) => {
		const x = Math.round(p.at.x * kx);
		const y = Math.round(p.at.y * ky);
		const w = Math.max(1, Math.round((p.at.x + p.at.w) * kx) - x);
		const h = Math.max(1, Math.round((p.at.y + p.at.h) * ky) - y);
		return { ...p, at: { x, y, w, h } };
	});
	return fits(out, to.cols, to.rows) ? out : arrangeEvenly(out, to.cols, to.rows, aspect);
}
