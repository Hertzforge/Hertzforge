import { describe, expect, it } from 'vitest';
import { treeOrder } from './order';

const listed = (names: [string, string][]) => names.map(([name, kind]) => ({ name, kind, groups: [], verbs: [] }) as never);

describe('the tree order', () => {
	it('lists each group together, the Basis first, members by kind, then the rest by kind', () => {
		const objects = listed([
			['Model Modes', 'shape_set'],
			['Geometry', 'geometry'],
			['Geometry (2)', 'geometry'],
			['FRF', 'frf'],
			['Table', 'channel_table'],
			['Time', 'time_history'],
			['Loose PSD', 'psd']
		]);
		const order = treeOrder(objects, [
			{ members: ['Model Modes', 'Geometry (2)'], role: null, name: 'FEM' },
			{ members: ['FRF', 'Geometry', 'Table'], role: 'Basis', name: '' }
		]).map((o) => o.name);
		expect(order).toEqual(['Geometry', 'Table', 'FRF', 'Geometry (2)', 'Model Modes', 'Time', 'Loose PSD']);
	});

	it('without links it is the kinds in order, as they arrived within a kind', () => {
		const objects = listed([
			['B', 'frf'],
			['A', 'geometry'],
			['C', 'frf']
		]);
		expect(treeOrder(objects, undefined).map((o) => o.name)).toEqual(['A', 'B', 'C']);
	});

	it('an object named in two groups is listed once, where it first appears', () => {
		const objects = listed([
			['X', 'psd'],
			['Y', 'psd']
		]);
		const order = treeOrder(objects, [
			{ members: ['X'], role: null, name: '' },
			{ members: ['X', 'Y'], role: null, name: '' }
		]).map((o) => o.name);
		expect(order).toEqual(['X', 'Y']);
	});
});
