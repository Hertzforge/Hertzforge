import { describe, expect, it } from 'vitest';
import { BASIS_COLOR, CURVE_COLORS, spans, ticks } from './brackets';
import { treeOrder } from './order';

const order = ['Geometry', 'Table', 'Time', 'PSD', 'FRF', 'Modes', 'FEM Modes'];

describe('the link brackets', () => {
	it('the Basis is blue and bold, other groups take the rest of the palette', () => {
		const painted = spans(
			[
				{ members: ['Modes', 'FEM Modes'], role: null, name: '' },
				{ members: ['Geometry', 'Table', 'Time'], role: 'Basis', name: 'Survey' }
			],
			order
		);
		expect(painted.map((s) => s.rows)).toEqual([
			['Geometry', 'Table', 'Time'],
			['Modes', 'FEM Modes']
		]);
		expect(painted[0]).toMatchObject({ color: BASIS_COLOR, bold: true, name: 'Survey', column: 0 });
		expect(painted[1]).toMatchObject({ color: CURVE_COLORS[1], bold: false, column: 1 });
	});

	it('a group with one row showing, or a member gone, paints nothing', () => {
		expect(spans([{ members: ['Gone', 'Time'], role: null, name: '' }], order)).toEqual([]);
		expect(spans(undefined, order)).toEqual([]);
	});

	it('colors stay with their groups when one above dissolves', () => {
		const a = spans(
			[
				{ members: ['Table', 'Time'], role: null, name: '' },
				{ members: ['FRF', 'Modes'], role: null, name: '' }
			],
			order
		);
		expect(a[1].color).toBe(CURVE_COLORS[2]);
		expect(a[1].column).toBe(1);
	});


	it('a Matched Modes row is striped in both its sets\' group colors, and follows the first set\'s group', () => {
		// it belongs to both groups (Brandon, 2026-11-14)
		const links = [
			{ members: ['Geometry', 'Test Modes'], role: 'Basis', name: '' },
			{ members: ['FEM', 'FEM Modes'], role: null, name: '' }
		];
		const objects = [
			{ name: 'Geometry', kind: 'geometry', groups: [] },
			{ name: 'Test Modes', kind: 'shape_set', groups: [] },
			{ name: 'FEM', kind: 'geometry', groups: [] },
			{ name: 'FEM Modes', kind: 'shape_set', groups: [] },
			{ name: 'Loose', kind: 'frf', groups: [] },
			{ name: 'Matched', kind: 'matched_modes', groups: [], between: ['Test Modes', 'FEM Modes'] as [string, string] }
		];
		expect(ticks(links, objects)).toEqual([{ row: 'Matched', colors: [BASIS_COLOR, CURVE_COLORS[1]] }]);
		expect(treeOrder(objects, links).map((o) => o.name)).toEqual(['Geometry', 'Test Modes', 'Matched', 'FEM', 'FEM Modes', 'Loose']);
		// one group holding both sets: one color; neither set grouped: no tick, and the row with the rest
		const one = [{ members: ['Test Modes', 'FEM Modes'], role: null, name: '' }];
		expect(ticks(one, objects)).toEqual([{ row: 'Matched', colors: [CURVE_COLORS[1]] }]);
		const none = [{ members: ['Geometry', 'FEM'], role: null, name: '' }];
		expect(ticks(none, objects)).toEqual([]);
		expect(treeOrder(objects, none).map((o) => o.name).slice(0, 2)).toEqual(['Geometry', 'FEM']);
		// in a group of its own, its group's bracket marks it
		const own = [...links, { members: ['Matched', 'Loose'], role: null, name: '' }];
		expect(ticks(own, objects)).toEqual([]);
	});
});
