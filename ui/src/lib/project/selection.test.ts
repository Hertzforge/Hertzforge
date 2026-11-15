import { describe, expect, it } from 'vitest';
import { EMPTY, clickObject, clickPart, clickRecord, partForService, partIsPicked, pruned, rectangle, renamed } from './selection';

const order = ['Geometry', 'Table', 'Time', 'FRF', 'Modes'];

describe('selecting objects', () => {
	it('a plain click selects only what was clicked, and releases record picks', () => {
		const picked = clickRecord(EMPTY, 'FRF', 3);
		expect(picked).toMatchObject({ objects: ['FRF'], records: { FRF: [3] } });
		const s = clickObject(picked, 'FRF', order);
		expect(s.objects).toEqual(['FRF']);
		expect(s.records).toEqual({});
	});

	it('Cmd adds and removes, keeping the order picked', () => {
		let s = clickObject(EMPTY, 'Modes', order);
		s = clickObject(s, 'Geometry', order, { toggle: true });
		expect(s.objects).toEqual(['Modes', 'Geometry']);
		s = clickObject(s, 'Modes', order, { toggle: true });
		expect(s.objects).toEqual(['Geometry']);
	});

	it('Shift extends from the anchor through the rows between', () => {
		let s = clickObject(EMPTY, 'Table', order);
		s = clickObject(s, 'FRF', order, { extend: true });
		expect(s.objects).toEqual(['Table', 'Time', 'FRF']);
		s = clickObject(s, 'Geometry', order, { extend: true });
		expect(s.objects).toEqual(['Geometry', 'Table']);
	});
});

describe('picking records', () => {
	const grid = [
		[0, 1],
		[2, -1],
		[3, 4]
	];

	it('a cell stands for its object; Cmd toggles cells', () => {
		let s = clickRecord(EMPTY, 'FRF', 0);
		s = clickRecord(s, 'FRF', 4, {}, undefined);
		expect(s.records).toEqual({ FRF: [4] });
		s = clickRecord(s, 'FRF', 0, { toggle: true });
		expect(s.records.FRF).toEqual([4, 0]);
		s = clickRecord(s, 'FRF', 4, { toggle: true });
		expect(s.records.FRF).toEqual([0]);
	});

	it('Shift picks the rectangle of cells, holes left out', () => {
		expect(rectangle(grid, 0, 4)).toEqual([0, 1, 2, 3, 4]);
		expect(rectangle(grid, 1, 2)).toEqual([0, 1, 2]);
		let s = clickRecord(EMPTY, 'FRF', 0);
		s = clickRecord(s, 'FRF', 3, { extend: true }, { from: 0, to: 3, grid });
		expect(s.records.FRF).toEqual([0, 2, 3]);
	});

	it('Cmd on a record of another object keeps the geometry beside it', () => {
		let s = clickObject(EMPTY, 'Geometry', order);
		s = clickRecord(s, 'Time', 2, { toggle: true });
		expect(s.objects).toEqual(['Geometry', 'Time']);
		expect(s.records).toEqual({ Time: [2] });
	});

	it('removing the last pick releases the object’s records', () => {
		let s = clickRecord(EMPTY, 'Time', 2);
		s = clickRecord(s, 'Time', 2, { toggle: true });
		expect(s.records).toEqual({});
		expect(s.objects).toEqual(['Time']);
	});
});

describe('parts and the project changing', () => {
	it('a part of a geometry is picked with it', () => {
		const s = clickPart(EMPTY, 'Geometry', 'family', 'quads');
		expect(s).toMatchObject({ objects: ['Geometry'], part: { name: 'Geometry', kind: 'family', id: 'quads' } });
	});

	it('what is gone leaves the selection, and a rename follows', () => {
		let s = clickObject(EMPTY, 'FRF', order);
		s = clickRecord(s, 'Time', 1, { toggle: true });
		const same = pruned(s, order);
		expect(same).toBe(s);
		const less = pruned(s, ['Geometry', 'Time']);
		expect(less.objects).toEqual(['Time']);
		expect(less.records).toEqual({ Time: [1] });
		expect(less.anchor).toBe('Time');
		const moved = renamed(s, 'Time', 'Run 1');
		expect(moved.objects).toEqual(['FRF', 'Run 1']);
		expect(moved.records).toEqual({ 'Run 1': [1] });
	});
});

describe('element groups picked together', () => {
	const order = [1, 2, 3, 4];
	const one = clickPart(EMPTY, 'Plate', 'group', 2);

	it('Cmd adds a group of the same geometry and takes it away again', () => {
		const two = clickPart(one, 'Plate', 'group', 4, { toggle: true }, order);
		expect(two.part).toEqual({ name: 'Plate', kind: 'group', id: 2, ids: [2, 4] });
		expect(partIsPicked(two, 'Plate', 'group', 4)).toBe(true);
		expect(partForService(two)).toEqual({ kind: 'group', id: 2, ids: [2, 4] });
		const back = clickPart(two, 'Plate', 'group', 4, { toggle: true }, order);
		expect(back.part).toEqual({ name: 'Plate', kind: 'group', id: 2 });
		expect(partForService(back)).toEqual({ kind: 'group', id: 2 });
	});

	it('Shift picks the run from the last group picked plainly', () => {
		const run = clickPart(one, 'Plate', 'group', 4, { extend: true }, order);
		expect(run.part?.ids).toEqual([2, 3, 4]);
		expect(run.part?.id).toBe(2);
	});

	it('a plain click, another kind of part or another geometry starts again', () => {
		const two = clickPart(one, 'Plate', 'group', 3, { toggle: true }, order);
		expect(clickPart(two, 'Plate', 'group', 1, {}, order).part).toEqual({ name: 'Plate', kind: 'group', id: 1 });
		expect(clickPart(two, 'Plate', 'family', 'hexes', { toggle: true }).part).toEqual({ name: 'Plate', kind: 'family', id: 'hexes' });
		expect(clickPart(two, 'Frame', 'group', 3, { toggle: true }, order).part).toEqual({ name: 'Frame', kind: 'group', id: 3 });
	});

	it('taking the last group away leaves the geometry selected', () => {
		const none = clickPart(one, 'Plate', 'group', 2, { toggle: true }, order);
		expect(none.part).toBeNull();
		expect(none.objects).toEqual(['Plate']);
	});
});

describe('solids picked together', () => {
	const one = clickPart(EMPTY, 'Frame', 'solid', 1);

	it('Cmd adds a solid of the same geometry, as the service reads it', () => {
		const two = clickPart(one, 'Frame', 'solid', 3, { toggle: true }, [1, 2, 3]);
		expect(partForService(two)).toEqual({ kind: 'solid', id: 1, ids: [1, 3] });
		expect(partIsPicked(two, 'Frame', 'solid', 3)).toBe(true);
	});

	it('Shift picks the run, and a solid with a group is a new pick', () => {
		expect(clickPart(one, 'Frame', 'solid', 3, { extend: true }, [1, 2, 3]).part?.ids).toEqual([1, 2, 3]);
		expect(clickPart(one, 'Frame', 'group', 2, { toggle: true }).part).toEqual({ name: 'Frame', kind: 'group', id: 2 });
	});
});

describe('a list of sub-items', () => {
	// Brandon, 2026-11-11: Shift picks a run of modes, as it does of any sub-items
	it('takes the run from the last one clicked through the one Shift-clicked, a list being a grid one column wide', () => {
		const column = [0, 1, 2, 3, 4, 5].map((i) => [i]);
		let s = clickRecord(EMPTY, 'Modes', 1);
		s = clickRecord(s, 'Modes', 4, { extend: true }, { from: 1, to: 4, grid: column });
		expect(s.records.Modes).toEqual([1, 2, 3, 4]);
		s = clickRecord(s, 'Modes', 0, { extend: true }, { from: 1, to: 0, grid: column });
		expect([...s.records.Modes].sort()).toEqual([0, 1, 2, 3, 4]);
	});
});
