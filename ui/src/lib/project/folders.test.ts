import { describe, expect, it } from 'vitest';
import type { ProjectSummary } from '$lib/protocol/types';
import { folderLabel, historyFolders } from './folders';
import { spans } from './brackets';
import { treeOrder } from './order';

type Listed = ProjectSummary['objects'][number];
const obj = (name: string, kind: string) => ({ name, kind }) as unknown as Listed;

const objects = [
	obj('Geometry', 'geometry'),
	obj('Test 1X+', 'time_history'),
	obj('Test 2X+', 'time_history'),
	obj('Model 1X+', 'time_history'),
	obj('Model 2X+', 'time_history'),
	obj('Run', 'time_history'),
	obj('Run Table', 'channel_table')
];
const links: NonNullable<ProjectSummary['links']> = [
	{ members: ['Test 1X+', 'Test 2X+'], role: null, name: 'test.unv' },
	{ members: ['Model 1X+', 'Model 2X+'], role: null, name: '' },
	{ members: ['Run', 'Run Table'], role: null, name: 'run3.mat' }
];
const order = treeOrder(objects, links).map((o) => o.name);

describe('group rows for time histories', () => {
	it('a group of time histories alone gets a row, its members in the tree order', () => {
		const folders = historyFolders(objects, links, order);
		expect(folders.map((f) => f.members)).toEqual([
			['Test 1X+', 'Test 2X+'],
			['Model 1X+', 'Model 2X+']
		]);
		expect(folders.map(folderLabel)).toEqual(['test.unv', 'Time histories']);
	});

	it('a group that holds anything besides time histories keeps its bracket and gets no row', () => {
		const folders = historyFolders(objects, links, order);
		expect(folders.some((f) => f.members.includes('Run'))).toBe(false);
		const painted = spans(links, order, new Set(folders.map((f) => f.group)));
		expect(painted.map((s) => s.name)).toEqual(['run3.mat']);
	});

	it('a group row keeps the other groups in their own colors', () => {
		const all = spans(links, order);
		const folders = historyFolders(objects, links, order);
		const left = spans(links, order, new Set(folders.map((f) => f.group)));
		expect(left[0].color).toBe(all.find((s) => s.name === 'run3.mat')!.color);
	});

	it('an unnamed group is kept open or closed by its first member, a named one by its name', () => {
		const folders = historyFolders(objects, links, order);
		expect(folders.map((f) => f.key)).toEqual(['test.unv', 'Model 1X+']);
	});

	it('a group of one, a member the summary does not list, or members apart in the order get no row', () => {
		expect(historyFolders(objects, [{ members: ['Run'], role: null, name: 'one' }], order)).toEqual([]);
		expect(historyFolders(objects, [{ members: ['Test 1X+', 'Gone'], role: null, name: '' }], order)).toEqual([]);
		const apart = ['Test 1X+', 'Geometry', 'Test 2X+'];
		expect(historyFolders(objects, [{ members: ['Test 1X+', 'Test 2X+'], role: null, name: '' }], apart)).toEqual([]);
	});
});
