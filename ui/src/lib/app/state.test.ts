import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AppState } from '$lib/app/state.svelte';
import { readConfig } from '$lib/app/config';
import { RequestError } from '$lib/protocol/client.svelte';
import type { ActionColumn, ActionRow, OperationDescriptor, ProjectSummary } from '$lib/protocol/types';

// The real state class with the service stubbed. `followPreview()` is called by hand where the
// `$effect` in Analyze.svelte would run it again: what it does given the state is proved here,
// that the effect runs it when the state changes is not.

const geometry = { name: 'geometry', label: 'Geometry', type: 'enum', options_from: 'objects:geometry' };
const addBlock = {
	id: 'project.add_solid_block',
	title: 'Add Block',
	workspace: 'geometry',
	modules: ['geometry'],
	previews: true,
	outputs: ['reading'],
	params: [geometry, { name: 'x', label: 'x', type: 'number', default: 0 }]
} as unknown as OperationDescriptor;
const editSolid = {
	id: 'project.edit_solid',
	title: 'Edit Solid',
	workspace: 'geometry',
	modules: ['geometry'],
	previews: true,
	outputs: ['reading'],
	params: [geometry, { name: 'length', label: 'Length', type: 'string', default: '' }]
} as unknown as OperationDescriptor;

const summary = {
	title: 't',
	file: null,
	modified: false,
	active_geometry: 'G',
	links: [],
	unresolved: [],
	objects: [
		{ name: 'G', kind: 'geometry', records: 0, groups: [], verbs: [] },
		{ name: 'H', kind: 'geometry', records: 0, groups: [], verbs: [] }
	]
} as unknown as ProjectSummary;

type Sent = { op?: string; params?: Record<string, unknown>; preview?: boolean };

function make() {
	const app = new AppState(readConfig('?analysis=ws://127.0.0.1:1&control=ws://127.0.0.1:1'));
	const runs: Sent[] = [];
	const answers = { refuse: '' };
	const request = vi.fn(async (method: string, params: Sent) => {
		if (method === 'op.run') {
			runs.push(params);
			if (answers.refuse) throw new RequestError('refused', answers.refuse, method);
			if (params.op === editSolid.id && !params.params?.length) throw new RequestError('refused', 'type what changes', method);
			return { views: [{ view: 'reading', title: params.preview ? 'PREVIEW' : 'REAL', rows: [] }], project: params.preview ? undefined : summary };
		}
		if (method === 'project.actions') return { table: { columns: [{ kinds: ['geometry'], current: true, rows: [] }] } };
		if (method === 'project.show') return { title: 'shown', views: [], note: '' };
		return {};
	});
	(app.analysis as unknown as { request: unknown }).request = request;
	app.catalog = { operations: [addBlock, editSolid] } as never;
	app.project.summary = summary;
	return { app, a: app.analyze, runs, answers };
}

const row = (op: OperationDescriptor) => ({ id: op.id, verb: op.title, kind: 'run', ready: true, fill: { geometry: 'G' } }) as unknown as ActionRow;
const column = { kinds: ['geometry'], current: true, rows: [] } as unknown as ActionColumn;
const said = (a: ReturnType<typeof make>['a']) => (a.reading?.views[0] as { title?: string } | undefined)?.title;
const rest = () => vi.advanceTimersByTimeAsync(400);

describe('a form that previews', () => {
	beforeEach(() => void vi.useFakeTimers());
	afterEach(() => void vi.useRealTimers());

	it('draws what it would do once the typing rests', async () => {
		const { a } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		expect(a.reading?.title).toBe('Add Block, preview');
		expect(said(a)).toBe('PREVIEW');
	});

	it('runs nothing once the form is closed, whatever the selection does', async () => {
		const { app, a, runs } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		a.closeForm();
		const before = runs.length;
		app.project.clickObject('H', ['G', 'H']);
		await vi.advanceTimersByTimeAsync(0);
		a.followPreview();
		await rest();
		expect(a.formOpen).toBe(false);
		expect(runs.length).toBe(before);
		expect(a.reading).toBeNull();
	});

	it('Back takes a preview off the stage', async () => {
		const { a } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		a.closeForm();
		expect(a.reading).toBeNull();
		expect(a.readingId).toBeNull();
	});

	it('Back leaves a reading that ran', async () => {
		const { a } = make();
		await a.perform(row(addBlock), column);
		await a.run();
		expect(said(a)).toBe('REAL');
		a.closeForm();
		expect(said(a)).toBe('REAL');
	});

	it('Back before the typing rests: nothing goes out', async () => {
		const { a, runs } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await vi.advanceTimersByTimeAsync(50);
		a.closeForm();
		await rest();
		expect(runs.length).toBe(0);
		expect(a.reading).toBeNull();
	});

	it('opened again, it draws again', async () => {
		const { a } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		a.closeForm();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		expect(said(a)).toBe('PREVIEW');
	});

	it('a Run inside the debounce is not overwritten by its own preview', async () => {
		const { a, runs } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		a.values = { ...a.values, [addBlock.id]: { ...a.currentValues, x: 5 } };
		a.followPreview();
		await vi.advanceTimersByTimeAsync(50);
		await a.run();
		a.followPreview();
		await rest();
		expect(said(a)).toBe('REAL');
		expect(runs.map((r) => r.preview ?? false)).toEqual([true, false]);
	});

	it('opening a form that waits for typing says nothing', async () => {
		const { a, runs } = make();
		await a.perform(row(editSolid), column);
		a.followPreview();
		await rest();
		expect(runs.length).toBe(1);
		expect(a.runError).toBeNull();
		expect(a.reading).toBeNull();
	});

	it('what was typed being refused says why, and takes the last preview down', async () => {
		const { a, answers } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		expect(said(a)).toBe('PREVIEW');
		answers.refuse = 'the width is above zero';
		a.values = { ...a.values, [addBlock.id]: { ...a.currentValues, x: -1 } };
		a.followPreview();
		await rest();
		expect(a.runError).toBe('the width is above zero');
		expect(a.reading).toBeNull();
	});
});

describe('a drag in a reading', () => {
	it('refused, draws the same views anew, so the handle goes back to where it is', async () => {
		const { a, answers } = make();
		await a.perform(row(addBlock), column);
		await a.run();
		const before = a.reading!.views[0];
		answers.refuse = 'not there';
		await a.pickInReading({ op: 'project.slide_coordinate_system', params: {} }, { distance: 1 });
		expect(a.runError).toBe('not there');
		expect(a.reading!.views[0]).not.toBe(before);
		expect(a.reading!.views[0]).toEqual(before);
	});
});
