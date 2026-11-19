import { describe, expect, it } from 'vitest';
import { landsBefore, reordered, treeOrder } from './order';

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

describe('where a dragged object lands', () => {
	const objects = listed([
		['Frame', 'geometry'],
		['Frame Modes', 'shape_set'],
		['Wing', 'geometry'],
		['Wing Modes', 'shape_set'],
		['Loose', 'geometry'],
		['Loose Time', 'time_history']
	]);
	const links = [
		{ members: ['Frame', 'Frame Modes'], role: null, name: '' },
		{ members: ['Wing', 'Wing Modes'], role: null, name: '' }
	];

	it('lands in the target group, by kind among its members', () => {
		expect(landsBefore(objects, links, ['Loose'], 'Wing Modes')).toBe('Wing Modes');
		expect(landsBefore(objects, links, ['Loose Time'], 'Frame')).toBe('Frame Modes');
	});

	it('leaves its group behind, which is no group with one member left', () => {
		const order = treeOrder(objects, [links[1], { members: ['Frame Modes', 'Loose'], role: null, name: '' }]).map((o) => o.name);
		expect(landsBefore(objects, links, ['Frame Modes'], 'Loose')).toBe(order[order.indexOf('Frame Modes') + 1] ?? null);
	});

	it('makes a new group last among the groups when the target has none', () => {
		expect(landsBefore(objects, links, ['Loose Time'], 'Loose')).toBeNull();
		expect(landsBefore(objects, links, ['Wing Modes'], 'Loose Time')).toBe('Wing');
	});

	it('lands among the loose objects when dragged out of its group, the bottom when last', () => {
		expect(landsBefore(objects, links, ['Wing Modes'], null)).toBeNull();
		expect(landsBefore(objects, links, ['Wing'], null)).toBe('Loose');
	});
});

describe('a solid dragged among its siblings', () => {
	it('lands before the row it was let go on, and last on null', () => {
		expect(reordered([1, 2, 3], 3, 1)).toEqual([3, 1, 2]);
		expect(reordered([1, 2, 3], 1, 3)).toEqual([2, 1, 3]);
		expect(reordered([1, 2, 3], 1, null)).toEqual([2, 3, 1]);
		expect(reordered([1, 2, 3], 2, 3)).toEqual([1, 2, 3]);
	});

	it('stays where it is when let go on its own row, or on the lower half of the row above', () => {
		expect(reordered([1, 2, 3], 2, 2)).toEqual([1, 2, 3]);
		expect(reordered([1, 2, 3, 4], 1, 1)).toEqual([1, 2, 3, 4]);
	});
});
