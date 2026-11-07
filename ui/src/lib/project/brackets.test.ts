import { describe, expect, it } from 'vitest';
import { BASIS_COLOR, CURVE_COLORS, spans } from './brackets';

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

});
