import { describe, expect, it } from 'vitest';
import { EMPTY, clickObject, clickPart, clickRecord, pruned, rectangle, renamed } from './selection';

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
